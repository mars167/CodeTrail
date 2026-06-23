# SRC KNOWLEDGE BASE

## OVERVIEW

`src/` is the Rust crate surface for the CLI, MCP server, query service, index storage, reliability model, graph, providers, and workspace proof logic.

## WHERE TO LOOK

| Task | Location | Notes |
| --- | --- | --- |
| Command definitions | `cli.rs` | Clap derives and aliases. |
| Command execution | `commands.rs` | Workspace setup, scan options, dispatch, output emission. |
| Output contract | `output.rs` | Caveats, reliability, paging, renderers. |
| Text/path queries | `search.rs` | Live scan, indexed candidates, broad guards, cursor scope. |
| Index and storage | `index.rs`, `lancedb_store.rs`, `snapshot_store.rs` | Freshness, manifests, LanceDB schemas, remote pack format. |
| Workspace proof | `workspace.rs`, `diff_proof.rs`, `watcher/` | File catalog, changed scope, dirty overlay, watcher status. |
| Semantic facts | `semantic_facts.rs`, `semantic_provider.rs`, `*_provider.rs` | Provider proof and partial/deferred work. |
| SCIP path | `scip_index.rs`, `scip_indexer.rs`, `scip/`, `scip_proto.rs` | Precise defs/refs/symbols and native/imported occurrence storage. |
| Calls/callers | `graph/`, `syntax.rs` | Petgraph and parser candidate fallbacks. |
| Programmatic facade | `query/mod.rs` | Good reference for using the same operations outside CLI dispatch. |

## CONVENTIONS

- Keep data provenance visible internally. Result producers, reliability, and warnings should reflect whether data came from local files, fresh index, parser fallback, graph, remote, or live overlay.
- Search-like commands should help locate evidence, not imply the file content was verified; exact source verification happens through the host editor or agent read tool.
- Cursor scopes include query args, scan options, and snapshot state; changing any should reject stale cursors rather than silently reusing them.
- `source_fact` and `precise_fact` can be exact. Parser and graph results stay candidate or fallback-level.
- Error JSON must still use the public shape with structured `error.code` and `error.message`; do not revive legacy envelopes.
- Keep stderr diagnostics separate from stdout renderers.

## ANTI-PATTERNS

- Do not bypass `output.rs` for public CLI/MCP-visible JSON.
- Do not add a shortcut that uses stale index data when proof fails.
- Do not blend `commit`, `staged`, and `worktree` into a single untraceable result set.
- Do not treat remote or saved query data as ground truth.
- Do not broaden `src/lib.rs` exports for convenience without checking downstream command/query boundaries.

## VERIFY

```bash
cargo fmt --check
cargo test --all-targets --locked --no-fail-fast
cargo test --test cli
```

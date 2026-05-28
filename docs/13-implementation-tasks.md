# Implementation Task Breakdown

> Status snapshot: 2026-05-28. This document tracks implementation progress against the current design docs.

## Progress Legend

- Done: implemented, tested, and committed.
- In progress: actively being implemented in the current follow-up.
- Pending: designed but not yet implemented.
- Deferred: intentionally not in the current local CLI scope.

## Task Breakdown

| Task | Status | Notes |
| --- | --- | --- |
| T1 CLI command surface | Done | `find`, `grep`, `files`, `find-path`, `glob`, `list`, `tree`, `read`, `refs`, `symbols`, `defs`, `calls`, `callers`, `changed`, `status`, `watch`, `serve`, `index`, and `hooks` are wired. |
| T2 Unified JSON reliability contract | Done | Responses include `snapshot_id`, `reliability`, `producer`, `exact`, warnings, and fallback metadata. |
| T3 L0 source fact commands | Done | Search, path, read, git status, and changed-file commands work without requiring a prebuilt index. |
| T4 Parser facts | Done | `symbols` and `defs` use tree-sitter fallback for Rust, Python, TypeScript, and JavaScript. |
| T5 Relation candidates | Done | `calls` and `callers` expose tree-sitter call heuristics as `inferred_candidate`, never `exact`. |
| T6 Index lifecycle | Done | Local `.code-search/index` JSONL cache, `build`, `update`, `status`, `verify`, and `clean` are implemented. |
| T7 Git hook lifecycle | Done | Hook install/status/uninstall support staged and commit update entrypoints without making hooks authoritative. |
| T8 Watch/serve status | Done | `watch --once`, `watch --status`, and `serve --no-watch` expose freshness/status contracts. |
| T9 Index-backed query path | Done | Fresh index file catalog feeds `files`, `find`, `grep`, and `refs` only when scan options match; matches still use live source verification. |
| T10 Shell completions | Done | `code-search completions bash|zsh|fish` prints built-in completion scripts without requiring a workspace. |
| T11 Precise SCIP integration | In progress | T11a SCIP JSON occurrence import is Done for `symbols`, `defs`, and `refs`; T11b binary `index.scip` protobuf parsing remains pending. |
| T12 Property graph backend | In progress | T12a local relation graph store is Done; T12b backend abstraction and T12c multi-hop impact traversal remain pending. Relation outputs stay `inferred_candidate`. |
| T13 MCP adapter | Pending | Should wrap the stable CLI query service after schema compatibility is locked. |
| T14 Remote index/graph mode | Deferred | Remote must never override local dirty/staged state; not part of this local MVP. |

## Completed Slices

1. T1-T10 are implemented, tested, committed, and pushed.
2. T11a SCIP JSON occurrence import is implemented, tested, committed, and pushed.
3. `symbols`, `defs`, and `refs` prefer fresh precise occurrence records and fall back only when the precise store is unavailable or stale.
4. T12a local relation graph store is implemented: `index build` writes relation records and `calls`/`callers` query the fresh store before parser fallback.

## Current Follow-Up Scope

1. Fresh index records now participate in query candidate selection when their manifest scan options match the query.
2. Source verification remains live, so the index improves candidate selection but does not become the fact source.
3. Shell completion generation is available without expanding runtime dependencies.
4. SCIP JSON occurrence import now populates precise local occurrence/declaration stores.
5. CLI integration tests cover index-backed query behavior, completion generation, precise SCIP JSON lookup, and graph-store relation lookup.

## Remaining Work

1. T11b: binary `index.scip` protobuf parsing for native SCIP files.
2. T12b: graph backend abstraction beyond JSONL relation records.
3. T12c: impact traversal and multi-hop graph queries.
4. T13: MCP adapter over the stable CLI schema.
5. T14: remote index/graph mode, intentionally deferred until local dirty/staged semantics are fully protected.

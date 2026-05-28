# Implementation Task Breakdown

> Status snapshot: 2026-05-28. This document tracks implementation progress against the current design docs.

## Progress Legend

- Done: implemented, tested, committed, and compliant with the target architecture.
- In progress: actively being implemented in the current follow-up.
- Pending: designed but not yet implemented.
- Deferred: intentionally not in the current local CLI scope.
- Non-compliant: implemented behavior exists, but it does not satisfy the target architecture and must be replaced.

## Task Breakdown

| Task | Status | Notes |
| --- | --- | --- |
| T1 CLI command surface | Done | `find`, `grep`, `files`, `find-path`, `glob`, `list`, `tree`, `read`, `refs`, `symbols`, `defs`, `calls`, `callers`, `changed`, `status`, `watch`, `serve`, `index`, and `hooks` are wired. |
| T2 Unified JSON reliability contract | Done | Responses include `snapshot_id`, `reliability`, `producer`, `exact`, warnings, and fallback metadata. |
| T3 L0 source fact commands | Done | Search, path, read, git status, and changed-file commands work without requiring a prebuilt index. |
| T4 Parser facts | Done | `symbols` and `defs` use tree-sitter fallback for Rust, Python, TypeScript, and JavaScript. |
| T5 Relation candidates | Done | `calls` and `callers` expose tree-sitter call heuristics as `inferred_candidate`, never `exact`. |
| T6 Index lifecycle | Non-compliant | Command entrypoints exist, but JSONL cache storage is not acceptable. Must be replaced by snapshots, gram segments, SCIP occurrence DB, and graph backend. |
| T7 Git hook lifecycle | Done | Hook install/status/uninstall support staged and commit update entrypoints without making hooks authoritative. |
| T8 Watch/serve status | Done | `watch --once`, `watch --status`, and `serve --no-watch` expose freshness/status contracts. |
| T9 Index-backed query path | Non-compliant | Query path currently relies on JSONL cache. Target requires text gram index, SCIP occurrence DB, and Kuzu graph query paths with live source verification. |
| T10 Shell completions | Done | `code-search completions bash|zsh|fish` prints built-in completion scripts without requiring a workspace. |
| T11 Precise SCIP integration | In progress | Native `index.scip` protobuf parsing and `occurrences.db` are required. SCIP JSON import is debug/import compatibility only and does not complete this task. |
| T12 Property graph backend | In progress | KuzuDB embedded backend is required. JSONL relation records are debug/export only and do not complete this task. Relation outputs stay `inferred_candidate`. |
| T13 MCP adapter | Pending | Should wrap the stable CLI query service after schema compatibility is locked. |
| T14 Remote index/graph mode | Deferred | Remote must never override local dirty/staged state; not part of this local MVP. |

## Completed Slices

1. T1-T5 are implemented, tested, committed, pushed, and architecture-compliant because they operate from live source/parser facts with explicit reliability labels.
2. T7-T8 are implemented at command/status level and remain valid lifecycle entrypoints.
3. T10 shell completions are implemented, tested, committed, and pushed.
4. The existing JSON/JSONL index code is not counted as completed architecture work.

## Current Follow-Up Scope

1. Replace JSONL file catalog with snapshot storage: `snapshots/<snapshot_id>/files.parquet` and content-addressed `blobs/`.
2. Replace live-scan/JSONL-backed search acceleration with `text/<snapshot_id>/grams.idx`, `docs.idx`, and `paths.idx`.
3. Replace SCIP JSON occurrence storage with native `scip/<snapshot_id>/index.scip` protobuf parsing and `occurrences.db`.
4. Replace JSONL relation records with `graph/<snapshot_id>/kuzu/`.
5. Keep JSON/JSONL only behind explicit export/debug/test-fixture paths.

## Remaining Work

1. T6a: source snapshot storage with `files.parquet` and `blobs/`.
2. T6b/T9: native gram/path index segments and query prefilter.
3. T11: binary `index.scip` protobuf parsing and occurrence DB.
4. T12: KuzuDB graph backend, backend trait, and impact traversal.
5. T13: MCP adapter over the stable CLI schema.
6. T14: remote index/graph mode, intentionally deferred until local dirty/staged semantics are fully protected.

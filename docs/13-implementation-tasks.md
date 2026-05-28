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
| T6 Index lifecycle | In progress | `index build/status/verify/clean` now writes `.code-search/snapshots/`, `.code-search/text/`, `.code-search/working/`, and `.code-search/staged/` with native text `.idx` segments. Source snapshot `files.parquet`/`blobs/`, SCIP occurrence DB, and graph backend remain required. |
| T7 Git hook lifecycle | Done | Hook install/status/uninstall support staged and commit update entrypoints without making hooks authoritative. |
| T8 Watch/serve status | Done | `watch --once`, `watch --status`, and `serve --no-watch` expose freshness/status contracts. |
| T9 Index-backed query path | In progress | `find`/`grep` use fresh `text/<snapshot>/grams.idx` as a candidate prefilter and verify matches from live files. Path-specific index lookup, SCIP occurrence DB, and Kuzu graph query paths remain required. |
| T10 Shell completions | Done | `code-search completions bash|zsh|fish` prints built-in completion scripts without requiring a workspace. |
| T11 Precise SCIP integration | In progress | SCIP JSON compatibility import now stores binary `scip/<snapshot>/occurrences.idx` and no JSONL. Native `index.scip` protobuf parsing plus `occurrences.db` are still required. |
| T12 Property graph backend | Pending | KuzuDB embedded backend is required. The previous JSONL relation store has been removed from `index build` and query dispatch; relation outputs stay tree-sitter `inferred_candidate` until Kuzu exists. |
| T13 MCP adapter | Pending | Should wrap the stable CLI query service after schema compatibility is locked. |
| T14 Remote index/graph mode | Deferred | Remote must never override local dirty/staged state; not part of this local MVP. |

## Completed Slices

1. T1-T5 are implemented, tested, committed, pushed, and architecture-compliant because they operate from live source/parser facts with explicit reliability labels.
2. T7-T8 are implemented at command/status level and remain valid lifecycle entrypoints.
3. T10 shell completions are implemented, tested, committed, and pushed.
4. T6b/T9 text index slice is implemented and tested: `index build` writes `text/<snapshot>/{docs.idx,paths.idx,grams.idx}`, `find`/`grep` use `grams.idx` for literal candidate prefilter, and `index verify` checks live file hashes before query reuse.
5. SCIP JSON compatibility import no longer uses JSONL storage; it writes binary `occurrences.idx`, but it is not counted as complete native SCIP architecture.

## Current Follow-Up Scope

1. Replace JSONL file catalog with snapshot storage: `snapshots/<snapshot_id>/files.parquet` and content-addressed `blobs/`.
2. Finish text index coverage beyond the completed literal content prefilter: path index lookup, regex prefilter planning, line-offset storage in `docs.idx`, and incremental segment merge/compaction.
3. Replace SCIP JSON compatibility import with native `scip/<snapshot_id>/index.scip` protobuf parsing and `occurrences.db`.
4. Replace JSONL relation records with `graph/<snapshot_id>/kuzu/`.
5. Keep JSON/JSONL only behind explicit export/debug/test-fixture paths.

## Remaining Work

1. T6a: source snapshot storage with `files.parquet` and `blobs/`.
2. T6b/T9 follow-up: path index lookup, regex prefilter, line-offset table, and incremental segment merge/compaction.
3. T11: binary `index.scip` protobuf parsing and occurrence DB.
4. T12: KuzuDB graph backend, backend trait, and impact traversal.
5. T13: MCP adapter over the stable CLI schema.
6. T14: remote index/graph mode, intentionally deferred until local dirty/staged semantics are fully protected.

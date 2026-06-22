# Agent Usage

This document keeps the longer CodeTrail agent guidance outside the default
skill prompt. The installed skill should stay small; agents can open this file
only when they need details beyond the routing card.

## Boundary

CodeTrail is now a semantic index frontend. Use it only for symbol lookup,
definition lookup, precise SCIP references, and call/caller candidates.

Use normal agent tools for everything else:

- text search: `rg`
- file discovery: `fd`, shell globbing, or host glob tools
- source verification: host read/editor tools
- Git state: `git`

Do not start repository exploration with CodeTrail. CodeTrail evidence is
navigation evidence; host reads still verify exact source before edits.

## Allowed Commands

```bash
codetrail --output json symbols <query>
codetrail --output json defs <identifier>
codetrail --output json refs <identifier>
codetrail --output json calls <identifier>
codetrail --output json callers <identifier>
codetrail --output json index status
codetrail --output json index doctor
```

Run `index doctor` only when a semantic query reports a missing or stale SCIP
index, or when deciding which SCIP provider setup is blocking precision.

`refs` is precise-reference only. If it returns
`precise_scip_index_unavailable`, the answer is "no usable precise reference
index"; use `rg` outside CodeTrail for textual occurrences.

## Disallowed Agent Usage

Do not call CodeTrail for:

- broad exploration or planning;
- text/path search wrappers: `find`, `grep`, `files`, `find-path`, `glob`;
- route discovery;
- source reads or directory browsing: `read`, `list`, `tree`;
- Git/worktree workflows: `changed`, `status`, `watch`, `serve`;
- saved-query replay;
- `explore node`.

These legacy/internal commands may still exist in the binary for tests or
compatibility, but they are not part of the agent-facing strategy.

## Evidence Package

Subagents should return compact evidence:

- `evidence` no more than 6 items.
- `relationships` no more than 8 items.
- `queries` no more than 8 items.
- Prefer no more than 5 CodeTrail commands total.
- Every evidence location must be `path:line` or `path:start-end`.
- File-only paths are leads, not evidence.

## Reliability

- `precise_fact`: SCIP occurrence fact; still verify source before editing.
- `parser_fact`: tree-sitter syntax fallback for symbols/definitions; not
  semantic reference proof.
- `inferred_candidate`: heuristic or graph relationship; verify before relying.

## Provider Notes

Provider details belong in command output and docs, not in the default skill.
For Kotlin, CodeTrail uses `scip-java index` with `CODETRAIL_SCIP_KOTLIN`
first and `CODETRAIL_SCIP_JAVA` as fallback. If precise setup is missing,
Kotlin falls back to `tree_sitter_parser` for symbols/definitions only.

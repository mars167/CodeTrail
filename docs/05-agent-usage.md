# Agent Usage

This document keeps the longer CodeTrail agent guidance outside the default
skill prompt. The installed skill should stay small; agents can open this file
only when they need details beyond the routing card.

## Boundary

CodeTrail is a search and navigation layer. It returns source, path, symbol,
reference, route, call-candidate, freshness, and status facts. It does not
replace host source reads before edits, and it should not become a planning or
architecture-analysis command surface.

Use host editor/agent reads to verify every important `path:line` or
`path:start-end` before editing.

## Low-Token Workflow

For multi-step repository investigations:

```bash
codetrail --output json index status --summary
```

Choose the cheapest matching primitive after preflight: `routes` for endpoints
and handlers, `defs` or `symbols` for known identifiers, `refs`/`calls`/
`callers` only for relevant names, and one bounded path/text discovery when
names are unknown.

Use compact node exploration only when one necessary symbol or path anchor
remains ambiguous:

```bash
codetrail --output json explore node <name> --compact --max-candidates 2 --snippet-lines 8 --relation-limit 4 --max-bytes 8000
```

Use one narrow supplement only when still needed:

```bash
codetrail --output json defs <name> --limit 10
codetrail --output json symbols <name> --limit 10
codetrail --output json refs <name> --limit 20
codetrail --output json calls <name> --limit 20
codetrail --output json callers <name> --limit 20
codetrail --output json routes <term> --limit 20
```

Use `files`, `find-path`, or `glob` for path discovery. Use `find` or `grep`
only for literal-text tasks, missing/stale indexes, unsupported languages, or
when navigation commands return no useful candidates.

## Evidence Package

Subagents should return compact evidence:

- `evidence` no more than 6 items.
- `relationships` no more than 8 items.
- `queries` no more than 10 items.
- Prefer no more than 6 CodeTrail commands total for one evidence package.
- Every evidence location must be `path:line` or `path:start-end`.
- Record fallback reasons for text search, non-index tools, stale indexes, or
  unsupported languages.

## Reliability

- `precise_fact`: SCIP occurrence fact; still verify source before editing.
- `parser_fact`: tree-sitter syntax fact; not semantic reference proof.
- `inferred_candidate`: heuristic or graph relationship; verify before relying.
- `source_fact`: filesystem/text/path fact; use exact host reads for edits.

## Provider Notes

Provider details belong in command output and docs, not in the default skill.
For Kotlin, CodeTrail uses `scip-java index` with `CODETRAIL_SCIP_KOTLIN`
first and `CODETRAIL_SCIP_JAVA` as fallback. If precise setup is missing,
Kotlin falls back to `tree_sitter_parser`.

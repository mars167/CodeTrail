---
name: codetrail
description: Use when searching, navigating, validating, or documenting the CodeTrail repository with the local codetrail CLI; especially for reliability-labeled source evidence, index freshness, SCIP/parser fallback, or MCP/JSON command contracts.
---

# CodeTrail

Use `codetrail` for narrow, reliability-labeled repository evidence. Prefer
JSON for agent work. Verify important source ranges with the host editor or
agent read tool before editing.

## Boundary

CodeTrail is the search and navigation layer.

It can return:

- source/path facts;
- symbols, definitions, references, routes, calls, and callers;
- bounded `explore flow` and compact `explore node` evidence;
- index freshness and semantic provider status;
- reliability labels and caveats.

It should not:

- decide whether a task is complete;
- replace exact source reads before edits;
- become an architecture or domain-analysis command surface;
- invent commands such as `brief`, `context`, or `analyze-*`.

## Command Prefix

Prefer the installed binary:

```bash
codetrail <command> ...
```

When the binary is not installed, run from the repository root:

```bash
cargo run --quiet -- <command> ...
```

Use `--path <dir>` when the target checkout is not the current directory.

## Fast Path

For multi-step repository investigations:

```bash
codetrail --output json index status --summary
codetrail --output json explore flow "<feature or flow>" --max-nodes 8 --snippet-lines 8 --relation-limit 8 --max-bytes 12000
```

Use compact node exploration only when the flow bundle misses an obvious node:

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
codetrail --output json routes <regex> --mode regex --limit 20
```

Use path discovery to find names or reduce scope:

```bash
codetrail --output json files <substring> --limit 20
codetrail --output json find-path <substring> --limit 20
codetrail --output json glob '<pattern>' --limit 20
```

Use content search only when the task is literal text, the index is missing or
stale, the language is unsupported, candidate names are unknown, or navigation
returns no useful results:

```bash
codetrail --output json find <literal> --limit 20
codetrail --output json grep <regex> --limit 20
```

## Core Commands

- `index status --summary`: compact index and semantic coverage status.
- `explore flow <query>`: compact flow bundle with nodes, short snippets, and capped relationships.
- `explore node <query> --compact`: bounded defs -> symbols -> files exploration for one node.
- `defs <name>`: definition candidates; prefers SCIP.
- `symbols <name>`: symbol candidates; prefers SCIP.
- `refs <name>`: references; falls back to identifier-boundary text search.
- `routes <term> [--mode regex]`: framework route declarations; searches route path, handler, framework, method, file path, and language.
- `calls <name>` / `callers <name>`: inferred call candidates.
- `files`, `find-path`, `glob`: indexed path discovery.
- `find`, `grep`: content fallback.

## Scope Controls

- `--include`, `--exclude`, `--dir`, `--ext`, and `--lang` narrow the search surface.
- `--changed` limits to git changed files.
- `--limit`, `--cursor`, `--allow-broad`, and `--context` control output budget.
- `--input-mode strict` disables compatible symbol input expansion.
- `--output json|compact-json|jsonl|text` selects renderer.

`compact-json` is not status summary mode. Use `index status --summary` for
compact status.

## Reliability

- `precise_fact`: SCIP occurrence fact; verify source before editing.
- `parser_fact`: tree-sitter syntax fact; not semantic reference proof.
- `inferred_candidate`: heuristic or graph relationship; verify before relying.
- `source_fact`: filesystem/text/path fact; verify exact ranges before editing.

Treat parser fallback, text fallback, graph relations, stale indexes, remote
unverified snapshots, and broad-query samples as leads with caveats.

## Subagent Use

Delegate long repository investigations to the CodeTrail evidence subagent when
available. Ask for a compact package only:

- short answer-oriented summary;
- `evidence` no more than 6 items;
- `relationships` no more than 8 items;
- `queries` no more than 10 items;
- caveats for missing, stale, fallback, ambiguous, or inferred results.

Every evidence location must include a line or line range:

```text
src/lib.rs:12
src/lib.rs:12-40
```

File-only paths are leads, not evidence.

## Do Not

- Do not call nonexistent `codetrail read`, `codetrail list`, or `codetrail tree`.
- Do not treat `parser_fact` or `inferred_candidate` as `precise_fact`.
- Do not use `find`/`grep` before `explore flow` or compact `explore node` when likely names exist.
- Do not paste whole files into the conversation when a range or snippet is enough.
- Do not add fields to public JSON casually; the public shape is `results`, `page`, `caveats`.
- Do not load long provider tables or agent schemas by default.

Longer guidance lives in `docs/05-agent-usage.md`.

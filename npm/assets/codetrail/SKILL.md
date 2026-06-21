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
- compact `explore node` evidence;
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

## Search Strategy

For multi-step repository investigations, first run one compact preflight:

```bash
codetrail --output json index status --summary
```

Then choose the cheapest evidence command from the query shape. Prefer
`compact-json` after preflight unless you need full pagination metadata.

- Route, endpoint, handler, filter, or middleware questions: start with
  `routes` and narrow by path, framework term, directory, extension, or regex.
- Known class, function, method, interface, or identifier: start with `defs`
  or `symbols`, then use `refs`, `calls`, or `callers` only for the few names
  that matter.
- Unknown names: do one bounded discovery step with `find-path`, `files`,
  `glob`, or a scoped `find`/`grep`, then return to navigation commands.
- Config, templates, SQL/XML/YAML, generated files, or other non-code
  artifacts: use scoped path/text commands; semantic navigation may not cover
  those files.
- Ambiguous single-node anchors: use compact `explore node` only after cheaper
  commands cannot identify the path, and keep the first budget small.

Common narrow commands:

```bash
codetrail --output compact-json routes <term> --limit 10
codetrail --output compact-json routes <regex> --mode regex --limit 10
codetrail --output compact-json defs <name> --limit 5
codetrail --output compact-json symbols <name> --limit 5
codetrail --output compact-json refs <name> --limit 10
codetrail --output compact-json calls <name> --limit 10
codetrail --output compact-json callers <name> --limit 10
```

Discovery and text fallback commands:

```bash
codetrail --output compact-json files <substring> --limit 10
codetrail --output compact-json find-path <substring> --limit 10
codetrail --output compact-json glob '<pattern>' --limit 10
codetrail --output compact-json find <literal> --limit 10
codetrail --output compact-json grep <regex> --limit 10
```

Single-node exploration fallback:

```bash
codetrail --output compact-json explore node <name> --compact --max-candidates 2 --snippet-lines 3 --relation-limit 2 --max-bytes 5000
```

Increase to `--max-candidates 4`, `--snippet-lines 4`, or `--max-bytes 8000`
only when the first compact result proves the path but lacks enough evidence.

## Core Commands

- `index status --summary`: compact index and semantic coverage status.
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
- Do not invent broad flow commands; use cheap primitives and only bounded `explore node` for one ambiguous anchor.
- Do not use broad `find`/`grep` before navigation when likely symbols, routes, or paths exist.
- Do not paste whole files into the conversation when a range or snippet is enough.
- Do not add fields to public JSON casually; the public shape is `results`, `page`, `caveats`.
- Do not load long provider tables or agent schemas by default.

Longer guidance lives in `docs/05-agent-usage.md`.

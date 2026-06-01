---
name: code-search-cli
description: Use when searching, navigating, validating, or documenting the code-search-cli repository with the local code-search CLI; especially when an agent needs reliable source evidence, saved query replay, freshness-aware index results, remote snapshot verification, or MCP/JSON command contracts.
---

# code-search-cli

Use `code-search` for narrow, verifiable source evidence in this repository. Prefer JSON output for agent work, and verify important matches with `read` before editing.

## Command Prefix

Prefer the installed binary when available:

```bash
code-search <command> ...
```

When the binary is not installed, run through Cargo from the repository root:

```bash
cargo run --quiet -- <command> ...
```

Use `--path <dir>` when searching from outside the repository root or when the user points at a different checkout.

## Core Workflow

1. Start with the narrowest command that can answer the question:
   - `code-search find <literal>`
   - `code-search grep <regex>`
   - `code-search files <substring>`
   - `code-search glob '<pattern>'`
   - `code-search defs|refs|symbols <name>`
2. Inspect `reliability`, `index`, `warnings`, `suggestedReads`, and `nextActions`.
3. Before editing or making a strong claim, verify key ranges with `code-search read <path[:start-end]>`.
4. Treat `calls` and `callers` as `inferred_candidate`; inspect the returned ranges before relying on them.
5. Treat `remote_unverified` as a lead only; verify with local `read`.

## Scope Controls

Use global options to keep output useful:

- `--include`, `--exclude`, `--lang`, and `--changed` narrow the search surface.
- `--limit`, `--cursor`, `--allow-broad`, and `--context` control paging and output budget.
- `--output json|compact-json|jsonl|text` selects the response shape; use `json` unless a human-readable transcript is requested.
- `--save-query <name>` records replay metadata for repeated investigations.

## Saved Queries

Use saved queries for repeatable investigations, not as a fact store.

```bash
code-search find "needle" --include src --save-query needle-src
code-search query replay needle-src
code-search query replay needle-src --snapshot saved
code-search query show needle-src
code-search query list
code-search query delete needle-src
```

Saved queries live in `.code-search/queries/<name>.json` and store command, query, scope, snapshot, and cursor metadata. They do not store result bodies. If the current snapshot differs, default replay runs against the current workspace and warns; `--snapshot saved` rejects the mismatch.

## Index And Freshness

- `code-search index build` writes the primary LanceDB store at `.code-search/index.lance`.
- `code-search index status` and `code-search index verify` report freshness, stale files, and active snapshot state.
- Dirty worktrees can combine fresh indexed files with live overlay for changed files.
- `code-search index pack` and `code-search index unpack` support remote snapshot sharing under `.code-search/remote/`.

## Reliability Levels

- `source_fact`: filesystem, text/path, Git, or `read`; usable as evidence after range verification.
- `precise_fact`: SCIP occurrence result; still verify before editing.
- `parser_fact`: tree-sitter syntax fact; useful syntax evidence, not semantic proof.
- `inferred_candidate`: heuristic or graph candidate; must verify.
- `freshness`: cache or watcher state only.
- `remote_verified`: remote snapshot matches local file proofs; still verify key edits.
- `remote_unverified`: remote snapshot does not match local files; lead only.

## MCP And JSON Contracts

When validating MCP or machine-readable behavior, compare against the command contract rather than prose summaries:

- Inspect `docs/02-command-contract.md` for command families and JSON response expectations.
- Inspect `src/cli.rs` for current CLI argument definitions.
- Inspect `src/output.rs` and the relevant command module when response fields or reliability metadata are in question.

## Project Validation

Use the repository scripts as the source of truth:

```bash
scripts/quality-gate.sh pr
scripts/quality-gate.sh main
scripts/quality-gate.sh bench
```

`quick` aliases `pr`; `cli` aliases `main`; `full` runs main then bench.

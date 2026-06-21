---
name: codetrail
description: Use only for CodeTrail semantic index lookups: symbols, definitions, precise references, and call/caller relationships that ordinary bash search cannot answer reliably.
---

# CodeTrail

CodeTrail is a semantic index frontend, not an exploration substrate.

Use normal agent tools first for text, path, file, and Git workflows:

- text search: `rg`
- file discovery: `fd`, shell globbing, or the host glob tool
- source reads: the host editor or agent read tool
- Git state: `git status`, `git diff`, and related Git commands

Use CodeTrail only when the question specifically needs code-intelligence
navigation that bash search cannot provide cleanly:

```bash
codetrail --output json symbols <query>
codetrail --output json defs <symbol>
codetrail --output json refs <symbol>
codetrail --output json calls <caller>
codetrail --output json callers <callee>
codetrail --output json index status
codetrail --output json index doctor
```

## Rules

- Do not start repository exploration with CodeTrail.
- Do not call `codetrail find`, `grep`, `files`, `find-path`, `glob`,
  `read`, `list`, `tree`, `changed`, `watch`, `serve`, `query`, or
  `explore node`.
- Treat CodeTrail results as navigation leads. Verify source ranges with the
  host read/editor tool before editing or making a strong claim.
- `refs` is a precise-index command. If there is no fresh SCIP occurrence
  index, use `rg` for textual matches instead of asking CodeTrail to fake
  semantic references.
- `calls` and `callers` are candidate relationships; verify each returned
  source range before reasoning from it.
- Parser fallback is a degradation layer. Do not describe it as precise
  semantic evidence.

Reliability labels:

- `precise_fact`: SCIP-backed symbol, definition, or reference evidence.
- `parser_fact`: tree-sitter syntax fallback, not semantic proof.
- `inferred_candidate`: call/caller relationship candidate.

## Index Readiness

When a semantic query fails because the precise index is missing or stale, run:

```bash
codetrail --output json index doctor
```

Then either build/configure the SCIP provider or continue with native bash
tools. Do not loop through CodeTrail fallback commands.

## Boundary

CodeTrail should not:

- plan repository investigations;
- decide when a task is complete;
- summarize architecture or data models;
- replace `rg`, `fd`, `cat`, `git`, or host source reads;
- return source snippets as a substitute for editor verification.

The stable value is a narrow one: a well-installed SCIP-backed index for
definitions, references, symbols, and call-chain candidates.

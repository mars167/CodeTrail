---
name: codetrail
description: Use only for CodeTrail semantic index lookups: symbols, definitions, precise references, and call/caller relationships that ordinary bash search cannot answer reliably.
---

# CodeTrail

CodeTrail is a semantic index frontend, not an exploration substrate. Use your
normal tools (`rg`, `fd`, host read/editor, `git`) for text, path, file, and
Git workflows. Reach for CodeTrail only when the missing evidence is
code-intelligence structure that bash search cannot resolve cleanly:

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

- Do not start repository exploration with CodeTrail, and do not use it to
  plan, decide when a task is done, or summarize architecture.
- CodeTrail results are navigation leads. Verify ranges with your host
  read/editor tool before editing or making a strong claim.
- `refs` is precise-index only. If there is no fresh SCIP index, use `rg` for
  textual matches instead of asking CodeTrail to fake semantic references.
- `calls` and `callers` are candidate relationships; verify each range.
- When a query reports a missing or stale index, run `index doctor`, then build
  or configure the SCIP provider, or continue with native tools. Do not loop.

## Reliability

- `precise_fact`: SCIP-backed symbol, definition, or reference evidence.
- `parser_fact`: tree-sitter syntax fallback, not semantic proof.
- `inferred_candidate`: call/caller relationship candidate.

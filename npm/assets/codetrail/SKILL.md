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
codetrail --output json call-hierarchy <method> --direction incoming|outgoing|both
codetrail --output json index status
codetrail --output json index doctor
```

## Language Strategy

Pick the strongest available signal for the language before falling back to
text search:

| Language | Best first queries | Use `rg` when |
| --- | --- | --- |
| Java | `call-hierarchy`, `calls`, `callers`, then `defs`/`refs` for exact symbols. Use text output only when a human-readable call tree is requested. | The Java semantic index is missing/stale, annotation-generated behavior is not indexed, or the question is literal text/config. |
| Go | `defs`/`refs`/`symbols` with fresh `scip-go`; use `calls`/`callers` only as navigation candidates. | `scip-go` is unavailable/stale or selector behavior is ambiguous. |
| Rust | `defs`/`refs`/`symbols` with fresh rust-analyzer SCIP; verify macro-heavy code carefully. | Macro expansion, cfg/features, or provider readiness blocks precise facts. |
| TypeScript/JavaScript | `defs`/`refs`/`symbols` with fresh `scip-typescript`; scope with `--lang typescript` or `--lang javascript`. | Package setup or generated code makes the SCIP index stale or incomplete. |
| Swift | `defs`/`refs`/`symbols` when SourceKit-backed SCIP is fresh. | SourceKit is missing, timed out, or the target needs build settings CodeTrail cannot infer. |
| Ruby | `defs`/`refs`/`symbols` when `scip-ruby` is fresh; treat dynamic dispatch cautiously. | Metaprogramming or missing bundle context hides the target. |
| Python | `symbols`/`defs`/`calls`/`callers` are parser leads, not semantic proof. | Any reference question needs exact textual confirmation. |
| Kotlin | Prefer Java/JVM path and text/path search unless a fresh semantic index covers the file. | Cross-language Java/Kotlin calls, Gradle-generated sources, or compiler plugins are involved. |
| Config/routes | Use `routes` for framework endpoints and config facts; combine with `refs` only when a precise symbol exists. | The task is key/value text, YAML/TOML/XML structure, or broad config discovery. |

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

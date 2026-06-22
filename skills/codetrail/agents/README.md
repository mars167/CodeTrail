# CodeTrail Agent Templates

This directory contains agent-layer templates that use CodeTrail as a narrow
semantic-index tool. They are intentionally separate from the CLI/MCP command
surface.

CodeTrail owns:

- symbol and definition lookup;
- precise reference lookup when a fresh SCIP occurrence index exists;
- call and caller candidates;
- semantic index status and doctor output.

Host agents own:

- text search, path discovery, source reads, and Git workflows;
- deciding whether a query needs semantic index evidence at all;
- verifying every `path:line` before editing;
- stopping investigations and compressing evidence for the primary session.

Do not add task-specific CLI commands such as `brief`, `context`, or
`analyze-*` to CodeTrail. Do not route broad repository exploration through a
CodeTrail subagent. Use ordinary tools such as `rg`, `fd`, source reads, and
`git` first, then use CodeTrail only where symbol, reference, or call-chain
structure is the missing evidence.

## Codex

Install the Codex template by copying:

```text
skills/codetrail/agents/codex/codetrail-evidence.toml
```

to:

```text
~/.codex/agents/codetrail-evidence.toml
```

The template registers the `codetrail-evidence` subagent. Invoke it only for
narrow semantic questions, such as:

- "where is this symbol defined?"
- "does this identifier have precise SCIP references?"
- "what calls this method?"
- "what does this function call?"

The subagent may run `symbols`, `defs`, `refs`, `calls`, `callers`,
`index status`, and `index doctor`. It must not call CodeTrail `find`, `grep`,
`files`, `find-path`, `glob`, `routes`, `explore node`, `read`, `list`, `tree`,
`changed`, `watch`, `serve`, or `query`.

## OpenCode

Install the OpenCode template by copying:

```text
skills/codetrail/agents/opencode/codetrail-evidence.md
```

to:

```text
.opencode/agents/codetrail-evidence.md
```

or:

```text
~/.config/opencode/agents/codetrail-evidence.md
```

The OpenCode template follows the same semantic-only boundary as the Codex
template.

# CodeTrail Agent Templates

This directory contains agent-layer templates that use CodeTrail as a search
tool. They are intentionally separate from the CLI/MCP command surface.

CodeTrail owns indexed discovery and reliability metadata:

- text, path, symbol, reference, call-candidate, status, and freshness facts;
- output budgets, pagination, caveats, and reliability labels;
- source range targets that the host agent can verify with its read tool.

Subagents own:

- deciding which CodeTrail primitive or host verification tool to call next;
- stopping multi-step investigations;
- compressing evidence into a compact package for a primary agent;
- adapting generic evidence collection to architecture, data model, debugging,
  review, or implementation tasks.

Do not add task-specific CLI commands such as `brief`, `context`, or
`analyze-*` to CodeTrail. Add task behavior to agent templates, and keep
CodeTrail's public commands as composable search primitives.

## Codex

Install the Codex template by copying:

```text
skills/codetrail/agents/codex/codetrail-evidence.toml
```

to:

```text
~/.codex/agents/codetrail-evidence.toml
```

The template registers the `codetrail-evidence` subagent. It should be invoked
for repository investigations that would otherwise consume many turns of search
and read output in the primary session.

The subagent uses a low-token index-first workflow: check
`codetrail --output json index status --summary` once, then choose the cheapest
command from the query shape. Prefer `routes` for endpoints and handlers,
`defs`/`symbols` then `refs`/`calls`/`callers` for known identifiers, one
bounded `files`/`find-path`/`glob`/scoped text search when names are unknown,
and scoped text search for config/templates/SQL/XML/YAML or other non-code
artifacts. Use compact `explore node` only for one ambiguous anchor, such as
`--compact --max-candidates 2 --snippet-lines 3 --relation-limit 2 --max-bytes 5000`,
then increase modestly only when the compact result proves the path. `list`,
`tree`, and `read` are not CodeTrail CLI/MCP commands.

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

The template is a `mode: subagent` agent. It should be invoked for repository
investigations that would otherwise consume many turns of search and read
output in the primary session.

The subagent uses a low-token index-first workflow: check
`codetrail --output json index status --summary` once, then choose the cheapest
command from the query shape. Prefer `routes` for endpoints and handlers,
`defs`/`symbols` then `refs`/`calls`/`callers` for known identifiers, one
bounded `files`/`find-path`/`glob`/scoped text search when names are unknown,
and scoped text search for config/templates/SQL/XML/YAML or other non-code
artifacts. Use compact `explore node` only for one ambiguous anchor, such as
`--compact --max-candidates 2 --snippet-lines 3 --relation-limit 2 --max-bytes 5000`,
then increase modestly only when the compact result proves the path. `list`,
`tree`, and `read` are not CodeTrail CLI/MCP commands.

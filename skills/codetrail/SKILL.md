---
name: codetrail
description: Use when searching, navigating, validating, or documenting the CodeTrail repository with the local codetrail CLI; especially when an agent needs reliable source evidence, saved query replay, freshness-aware index results, remote snapshot verification, or MCP/JSON command contracts.
---

# CodeTrail

Use `codetrail` for narrow, verifiable source evidence in this repository. Prefer JSON output for agent work, use indexed commands for discovery, and verify important matches with an exact source read before editing.

## Boundary

CodeTrail is the search and navigation tool layer. It should not take over
task planning, decide when an investigation is complete, or produce final
task answers on its own.

For a single narrow lookup, call the CLI directly. For multi-step repository
investigations, delegate to a CodeTrail evidence subagent when one is
available, then use its compact evidence package in the main task. The
subagent owns query sequencing and compression; CodeTrail still only returns
source, navigation, relationship, status, and freshness facts. The subagent may
also use ordinary agent read/search tools for verification or gap-filling; do
not force it to use CodeTrail for every file read.

## Command Prefix

Prefer the installed binary when available:

```bash
codetrail <command> ...
```

When the binary is not installed, run through Cargo from the repository root:

```bash
cargo run --quiet -- <command> ...
```

Use `--path <dir>` when searching from outside the repository root or when the user points at a different checkout.

## Core Workflow

Use an index-first workflow for repository investigations. CodeTrail's design
is to use indexed navigation, text, and path evidence to shrink the search
space, then use exact source reads as the verification surface. Do not count
`read`, `list`, or `tree` as indexed search commands: they are filesystem
inspection and verification helpers. Do not let a multi-step investigation
degrade into broad `grep` plus repeated reads unless the index is missing,
stale, unsupported for the language, or the task is explicitly literal-text
or path-only.

1. For multi-step investigations, check freshness and semantic readiness first:
   - `codetrail --output json status`
   - `codetrail --output json index status`
2. Extract candidate names from the task: symbols, types, functions, methods,
   routes, config keys, file stems, or domain terms.
3. Start with the narrowest indexed/navigation command that can answer the
   question:
   - `codetrail symbols <name>`
   - `codetrail defs <name>`
   - `codetrail refs <name>`
   - `codetrail routes <pattern>`
   - `codetrail calls <caller-name>`
   - `codetrail callers <callee-name>`
4. Use indexed path commands to scope navigation, not as a replacement for it:
   - `codetrail files <substring>`
   - `codetrail find-path <substring>`
   - `codetrail glob '<pattern>'`
5. Use `find` and `grep` as fallbacks or for literal-text questions:
   - `codetrail find <literal>`
   - `codetrail grep <regex>`
   Record why a content-search fallback was needed.
6. Inspect `reliability`, `index`, `warnings`, `suggestedReads`, and `nextActions`.
   - Treat `severity=info, category=capability` as an expected capability-level note, not a risk warning.
   - Treat `severity=warning, category=risk` and `severity=error, category=error` as requiring narrowing, verification, or remediation.
7. Before editing or making a strong claim, verify source with an exact file
   read. Use the host agent's read tool when available; `codetrail read
   <path[:start-end]>` remains available when you need the CLI's range parsing
   or JSON projection.
   - Prefer one whole-file read when the file is small enough to fit the output
     budget, or when you need several ranges from the same file.
   - Use `path:start-end` for known-large files, truncated full reads, or a
     single narrow verification.
8. Treat `calls` and `callers` as `inferred_candidate`; inspect the returned ranges before relying on them.
9. Treat `remote_unverified` as a lead only; verify with a local source read.

For architecture, data-model, refactor, debugging, and review tasks, make at
least two semantic/navigation attempts before the first content search when
candidate names are available. Good default pairs are `symbols` + `defs`,
`defs` + `refs`, `routes` + `refs`, or `defs` + `callers`. If no candidate
names are known, use an indexed path command to discover names, then return to
semantic/navigation commands.

## Fast Path Playbooks

Use these playbooks to keep common agent investigations short. They do not
replace source verification; they define the shortest useful query order.

### API And Domain Flow Investigations

Use this path for tasks such as analyzing login, user management, permissions,
or other web/API flows and producing a design summary or flow diagram.

1. Check index readiness once with `codetrail --output json index status`.
2. Locate ingress routes before content search:
   - `codetrail --output json routes login --limit 30`
   - `codetrail --output json routes user --limit 50`
   - Add one route term for each domain term from the task.
3. Read only the route controller ranges that own the selected endpoints.
4. From controller fields, imports, parameters, annotations, and method calls,
   extract service, model, mapper/repository, security, and view/client names.
5. Resolve those names with semantic/navigation commands first:
   - `codetrail --output json symbols <ModelOrControllerName> --limit 20`
   - `codetrail --output json defs <serviceOrMapperMethod> --limit 20`
   - `codetrail --output json refs <routeOrServiceMethod> --limit 20`
6. If a Java service, mapper, XML mapper, template, or static client name is
   not found by symbols/defs, use `codetrail --output json files <ClassOrStem>
   --limit 40` as path discovery, then immediately verify with an exact source read.
7. Verify only cross-layer boundaries needed for the answer:
   - route/controller methods and annotations;
   - authentication realm, filter, or login service;
   - service methods that enforce validation, permissions, or transactions;
   - domain/model fields relevant to the task;
   - mapper/repository interface and SQL/XML for persistence;
   - templates or static API clients only when the user asks about UI behavior.
8. Stop once the evidence covers ingress, business decision points,
   persistence, model shape, and authorization/authentication boundaries.
   Do not read every getter, route variant, or helper after the flow is proven.
9. For diagrams, build Mermaid or another flow only from verified ranges.
   Every node or edge that asserts code behavior must map back to a
   `path:start-end` citation.

For a RuoYi-like Spring/Shiro task about user management and login, the
shortest useful query chain is:

```bash
codetrail --output json index status
codetrail --output json routes login --limit 30
codetrail --output json routes user --limit 50
codetrail --output json files SysUser --limit 40
codetrail --output json files Shiro --limit 40
# Verify the selected controller/service/model/mapper ranges with the host
# read tool or with `codetrail --output json read <path:start-end>`.
```

## Command Input Quick Reference

Search and navigation inputs have a few command-specific formats that are not
obvious from the CLI argument names:

- `find <text>` defaults to literal search. `grep <pattern>` defaults to Rust
  regex search. Content search accepts `--mode literal|regex|wildcard`.
- `files <pattern>` and `find-path <pattern>` default to path literal
  substring matching. `glob <pattern>` defaults to glob syntax such as
  `src/**/*.rs`. These indexed path commands accept `--mode
  literal|regex|wildcard|glob`.
- Use `--dir`, `--ext`, `--file-pattern`, and `--file-mode` to scope before
  scanning file contents or parsing symbols. Matching is ignore-case by
  default; add `--case-sensitive` when exact case matters.
- `list [dir]`, `tree [dir]`, and `read <target>` are filesystem/source
  verification commands, not indexed discovery commands. Prefer normal agent
  read/list tools for these operations when they are available. When using
  `codetrail read`, the target accepts `path`, `path:line`, or
  `path:start-end`; line numbers are 1-based.

Navigation and relationship commands take one string argument. They default to
`--input-mode compatible`, so simple names, `Class.method`, signature display
names, and snake/kebab style keys are accepted when they can be normalized.
Use `--input-mode strict` to match only the raw input.

- `refs <identifier>` finds references to that identifier. With SCIP it matches
  exact symbol names, SCIP symbols, symbol keys, and bare method names for
  signature display names such as `selectUserById(Long)`. Without SCIP it is
  identifier-boundary literal text search.
- `calls <caller-name>` finds outgoing calls made inside a function or method
  whose name matches `<caller-name>`.
- `callers <callee-name>` finds incoming callers of a callee. For parser
  fallback, pass the simple final identifier such as `helper`, not
  `self.helper`, `pkg.Helper`, or `obj.helper`.

For Go, Rust, Python, TypeScript, JavaScript, and Java parser fallback,
compatible input matching is done after symbols/calls are extracted. It does
not use edit-distance fuzzy matching. Member or selector calls may be returned
as qualified targets, but `callers` still queries by the final identifier.

Scope and workflow inputs:

- `--include` and `--exclude` are path substring filters, not globs.
- `--lang` is a case-insensitive language name derived from extension: `go`,
  `rust`, `python`, `java`, `typescript`, `javascript`, `markdown`, `json`,
  `toml`, `yaml`, `html`, `css`, or `text`.
- `--cursor` is opaque and must come from the same query scope and snapshot.
- `--save-query <name>` and `query replay|show|delete <name>` use names made
  only from ASCII letters, digits, `.`, `_`, and `-`; `.` and `..` are invalid.
- `index import-scip <path>` accepts either SCIP JSON or native binary
  `index.scip` protobuf and auto-detects by content. `index generate-scip`
  currently supports only `--lang go`.
- `index pack --output <path>` writes a `.tar.gz` archive; `--output -` writes
  the archive bytes to stdout. `index unpack <path>` expects that archive.

## Subagent Handoff

Use the repository's CodeTrail evidence subagent template for tasks that would
otherwise require a long loop of search/read/refine steps. Ask the subagent to
return only:

- the task it investigated;
- a short answer-oriented summary;
- path and line-range evidence;
- caveats about missing, ambiguous, stale, or inferred results;
- whether the semantic index was checked and which index-backed commands were
  tried before text search;
- a concise query trace.

Every evidence location returned by the subagent must include a line number or
line range such as `src/lib.rs:12-40`. File-only paths are leads, not evidence.

Do not ask the subagent to edit files or make product decisions. Do not ask
the CodeTrail CLI to run task-specific analysis commands such as `brief`,
`context`, `analyze architecture`, or `analyze data-model`.

## Scope Controls

Use global options to keep output useful:

- `--include`, `--exclude`, `--lang`, and `--changed` narrow the search surface.
- `--limit`, `--cursor`, `--allow-broad`, and `--context` control paging and output budget.
- `--output json|compact-json|jsonl|text` selects the response shape; use `json` unless a human-readable transcript is requested.
- `--save-query <name>` records replay metadata for repeated investigations.

## Saved Queries

Use saved queries for repeatable investigations, not as a fact store.

```bash
codetrail find "needle" --include src --save-query needle-src
codetrail query replay needle-src
codetrail query replay needle-src --snapshot saved
codetrail query show needle-src
codetrail query list
codetrail query delete needle-src
```

Saved queries live in `.codetrail/queries/<name>.json` and store command, query, scope, snapshot, and cursor metadata. They do not store result bodies. If the current snapshot differs, default replay runs against the current workspace and warns; `--snapshot saved` rejects the mismatch.

## Index And Freshness

- `codetrail index build` writes the primary LanceDB store at `.codetrail/index.lance`.
- `codetrail index status` and `codetrail index verify` report freshness, stale files, and active snapshot state.
- Dirty worktrees can combine fresh indexed files with live overlay for changed files.
- `codetrail index pack` and `codetrail index unpack` support remote snapshot sharing under `.codetrail/remote/`.

## Semantic Provider Readiness

`codetrail index build` may report semantic provider install help under
`results[0].index.semantic.providerInstallHelp`. Treat this as an index
readiness issue, not as a search failure.

Primary semantic providers:

| Language | Provider command | Override env var | Install hint |
| --- | --- | --- | --- |
| Go | `scip-go` | `CODETRAIL_SCIP_GO` | `go install github.com/scip-code/scip-go/cmd/scip-go@latest` |
| Rust | `rust-analyzer scip .` | `CODETRAIL_SCIP_RUST` | `rustup component add rust-analyzer` |
| Java | `scip-java index` | `CODETRAIL_SCIP_JAVA` | Install Coursier and run `mkdir -p "$HOME/.local/bin" && coursier bootstrap --standalone -o "$HOME/.local/bin/scip-java" com.sourcegraph:scip-java_2.13:0.12.3 --main com.sourcegraph.scip_java.ScipJava` |
| TypeScript/JavaScript | `scip-typescript index` | `CODETRAIL_SCIP_TYPESCRIPT` | `npm install -g @sourcegraph/scip-typescript` |
| Ruby | `scip-ruby .` | `CODETRAIL_SCIP_RUBY` | `bundle add scip-ruby --group development` |
| Swift | `sourcekit-lsp` | `CODETRAIL_LSP_SWIFT` | Install Xcode or a Swift toolchain that includes `sourcekit-lsp` |

If a provider is missing, failed, or timed out, continue with parser/text
fallback only as `parser_fact` or `inferred_candidate`, and verify with
`codetrail read` before editing. Do not describe fallback results as precise
semantic facts.

## Reliability Levels

- `source_fact`: filesystem, text/path, Git, or source reads; usable as evidence after range verification.
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

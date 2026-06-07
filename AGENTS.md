# PROJECT KNOWLEDGE BASE

**Generated:** 2026-06-07 19:30:45 +0800
**Commit:** 811f8f5
**Branch:** codex/init-deep-agents

## OVERVIEW

CodeTrail is a Rust CLI and MCP server for local-index-first code search with explicit reliability labels. Its core rule: search/index/graph answers are navigation evidence; `read` is the source verification surface before editing.

## STRUCTURE

```text
code-search-cli/
|-- src/                 # Rust crate: CLI, query service, index, output, MCP, graph, providers
|-- tests/               # Rust CLI contract and subsystem integration tests
|-- scripts/             # local/CI quality gates, benchmarks, installer checks
|-- proto/               # SCIP protobuf source compiled by build.rs
|-- scip-indexer/        # separate Go sidecar for Go SCIP-like index generation
|-- docs/                # architecture, command contract, quality map
|-- skills/codetrail/    # agent skill for using CodeTrail as evidence tooling
|-- build.rs             # prost build step for proto/scip.proto
|-- Cargo.toml           # Rust package, binary, library, dependencies
`-- rust-toolchain.toml  # Rust 1.95.0 with rustfmt and clippy
```

## WHERE TO LOOK

| Task | Location | Notes |
| --- | --- | --- |
| Add or change CLI args | `src/cli.rs` | Clap surface is canonical with `codetrail --help`. |
| Route command behavior | `src/commands.rs` | Dispatcher joins workspace, scan options, query/index backends, output. |
| Text/path/list/tree/read behavior | `src/search.rs` | Broad guards, pagination, live/index candidates, read verification. |
| Output JSON/text/JSONL contract | `src/output.rs`, `docs/02-command-contract.md` | Public JSON is `results`, `page`, `caveats`. |
| Index lifecycle and freshness | `src/index.rs`, `src/lancedb_store.rs`, `src/snapshot_store.rs` | Snapshot and file proof rules live here. |
| Dirty worktree/watch behavior | `src/watcher/`, `src/diff_proof.rs` | Watcher reconciles overlays; it must not stage files. |
| Precise defs/refs/symbols | `src/scip_index.rs`, `src/scip/`, `proto/scip.proto` | SCIP path is precise only when fresh and verified. |
| Calls/callers | `src/graph/`, `src/syntax.rs` | Always candidates, never semantic proof. |
| MCP transport | `src/mcp/` | Shares the same public projection as CLI JSON. |
| Project graph and config facts | `src/project_graph.rs`, `src/config_facts/` | Polyglot root ownership and config-derived facts. |
| Quality gate | `scripts/quality-gate.sh`, `docs/03-quality.md` | `pr`, `main`, `bench`, `full` are the maintained entrypoints. |

## CODE MAP

LSP rust-analyzer failed to start in this worktree; this map is scan-derived.

| Symbol or file | Type | Location | Role |
| --- | --- | --- | --- |
| `Cli`, `Command`, `OutputFormat` | clap types | `src/cli.rs` | Public command and output surface. |
| `commands::run` | dispatcher | `src/commands.rs` | Main runtime switchboard. |
| `search::find/read/files/list/tree` | query fns | `src/search.rs` | Source/path facts and verification reads. |
| `index::*` | lifecycle | `src/index.rs` | Build, update, verify, pack, unpack, hooks. |
| `QueryService` | facade | `src/query/mod.rs` | Programmatic path shared by integrations. |
| `output::*` | contract | `src/output.rs` | Rendering, caveats, reliability, paging. |
| `GraphStore`, `PetgraphBackend` | graph backend | `src/graph/mod.rs` | Call/caller candidates and persistence. |
| `SemanticProvider` | trait | `src/semantic_provider.rs` | Language provider scheduling and partials. |
| `ProjectGraph` | model | `src/project_graph.rs` | Root discovery, config/dependency edges. |
| `LanceDbStore` | storage | `src/lancedb_store.rs` | Main index storage schema. |

## CONVENTIONS

- Snapshot identity is a boundary: keep `commit`, `staged`, and `worktree` provenance separate.
- Public JSON shape is intentionally small: `results`, `page`, `caveats`; do not leak internal audit fields.
- Reliability is part of the API: `source_fact`, `precise_fact`, `parser_fact`, `inferred_candidate`, freshness and remote labels must stay distinct.
- Remote snapshots may accelerate or annotate confidence; they must not override local working or staged state.
- Saved queries store replay metadata and cursors, not result payloads.
- Verbose diagnostics go to stderr; stdout must stay clean for text, JSON, and JSONL consumers.
- If command contracts, reliability labels, indexing, remote, watcher, or MCP output change, update `docs/` and tests in the same change.

## ANTI-PATTERNS (THIS PROJECT)

- Do not mark parser fallback or call graph output as exact semantic facts.
- Do not mix stale index rows into current answers without fallback or a caveat.
- Do not add fields to public JSON casually; tests treat the shape as a stable contract.
- Do not let watcher or remote code modify staged files or working snapshots.
- Do not treat `.codetrail/`, `target/`, or benchmark outputs as hand-authored source.

## COMMANDS

```bash
cargo build
cargo test
scripts/quality-gate.sh pr
scripts/quality-gate.sh main
scripts/quality-gate.sh bench
```

Use `scripts/quality-gate.sh pr` for ordinary PR changes. Use `main` when release build or RuoYi smoke behavior matters. Use `bench` when touching performance-sensitive search, index, or startup paths.

## NOTES

- Toolchain is pinned to Rust `1.95.0`; `rustfmt` and `clippy` components are expected.
- Go code under `scip-indexer/` is a separate module and should be verified with Go tooling.
- `build.rs` compiles `proto/scip.proto`; generated Rust is included through `src/scip_proto.rs`.
- `tests/cli.rs` is large because it pins the external CLI/MCP-visible contract.

# MCP KNOWLEDGE BASE

## OVERVIEW

`src/mcp/` is the JSON-RPC stdio adapter. It must expose the same facts and public JSON projection as the CLI query surface.

## WHERE TO LOOK

| Task | Location | Notes |
| --- | --- | --- |
| MCP tool registration | `mod.rs` | Tool names, schemas, dispatch, responses. |
| JSON-RPC protocol structs | `protocol.rs` | Request/response/error envelope types. |
| Shared query behavior | `../query/mod.rs`, `../commands.rs` | CLI and MCP should not fork facts. |
| Output projection | `../output.rs` | Public result shape and caveat structure. |
| Contract tests | `../../tests/cli.rs` | Includes MCP stdio parity coverage. |

## CONVENTIONS

- MCP tool results should use the same public projection as `--output json`: `results`, `page`, `caveats`.
- Keep tool schemas stable and explicit. Schema-only changes can break clients even when Rust tests compile.
- Normalize command errors into structured caveats where possible; avoid raw internal error strings as machine contract.
- If a tool wraps search/index/graph behavior, preserve reliability labels from the underlying implementation.
- MCP stdout is protocol traffic. Diagnostics belong on stderr or structured responses, not loose prints.

## ANTI-PATTERNS

- Do not duplicate CLI query logic inside MCP handlers when a shared service or command path exists.
- Do not invent MCP-only field names for facts already represented by `output.rs`.
- Do not mark parser fallback, graph candidates, or remote-unverified results as precise for client convenience.
- Do not change tool names or parameter shapes without updating docs and contract tests.

## VERIFY

```bash
cargo test --test cli mcp
cargo test --all-targets --locked --no-fail-fast
```

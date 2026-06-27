---
name: codetrail
description: Use for low-token CodeTrail CLI lookups: framework routes, symbols, definitions, precise references, and call/caller relationships that plain text search cannot answer cleanly.
---

# CodeTrail

Use CodeTrail when you need structured navigation, not for broad repository
exploration. Use normal tools (`rg`, `fd`, host read/editor, `git`) for text,
paths, file reads, and Git workflows.

The CLI defaults to compact text output. Add `--output json` only when another
tool must parse the response.

```bash
codetrail routes <pattern> --framework <name> --method GET --lang <lang> --dir <src-dir> --limit 20
codetrail symbols <name> --lang <lang> --dir <src-dir> --limit 10
codetrail defs <symbol> --lang <lang> --file-pattern <file> --include-code --code-max-lines 20 --limit 3
codetrail call-hierarchy <symbol> --direction incoming|outgoing --depth 2 --lang <lang> --dir <src-dir> --limit 40
```

## Default Flow

1. For HTTP endpoint, handler, or route-entry questions, start with `routes`.
2. For ordinary symbols or methods, start with `symbols` or `defs`; always add
   `--lang`, `--dir` or `--file-pattern`, and a small `--limit`.
3. Then query call direction: who calls it is `--direction incoming`; what it
   calls is `--direction outgoing`. Keep `--depth 2` unless there is a clear
   reason to expand.
4. When implementation evidence is needed, use
   `defs`/`symbols --include-code --code-max-lines 8..30` instead of reading a
   whole file.
5. On empty results, dynamic dispatch, macros, trait/interface dispatch, or
   generated behavior, switch to `rg` and small source reads immediately.

## Routes First

Use `routes` to find HTTP routes, framework endpoints, and handler candidates.
It scans CLI-visible source files for Java Spring, Go gin/chi/gorilla/net/http,
Python Django/FastAPI, TypeScript/JavaScript Express/NestJS, Ruby Rails, and
Swift Vapor patterns.

```bash
codetrail routes --framework gin --method GET --lang go --dir . --limit 20
codetrail routes users --framework express --lang javascript --dir lib --limit 20
codetrail routes --framework rails --method GET --dir config --limit 20
```

After a route match, use the returned `handler` and `path` to narrow the next
query:

```bash
codetrail defs <handler> --lang <lang> --file-pattern <route-or-handler-file> --include-code --code-max-lines 20 --limit 3
codetrail call-hierarchy <handler> --direction outgoing --depth 2 --lang <lang> --dir <src-dir> --limit 40
```

`routes` is the CLI route scanner. Do not rely on legacy MCP route tools.

## Language Recipes

| Language | Start here | Fallback quickly when |
| --- | --- | --- |
| Go | Scope by package or file. `codetrail symbols Default --lang go --file-pattern gin.go --limit 5`; `codetrail defs Engine.addRoute --lang go --include-code --code-max-lines 20 --limit 3`; `codetrail call-hierarchy Engine.addRoute --direction incoming --lang go --depth 2 --limit 40`; route tasks start with `routes --framework gin\|chi\|gorilla\|net/http`. | Selector or interface dispatch is the key fact, or generated/test/vendor code pollutes results. |
| Rust | Scope by crate or exact file. `codetrail symbols Config --lang rust --dir crates/core --limit 30`; `codetrail defs run --lang rust --file-pattern crates/core/main.rs --include-code --code-max-lines 25 --limit 3`; `codetrail call-hierarchy search --lang rust --dir crates/core --direction outgoing --depth 2 --limit 40`. | Macros, cfg/features, trait dispatch, or broad names like `run`, `search`, `new` need source confirmation. |
| TypeScript/JavaScript | Always set `--lang typescript` or `--lang javascript` and scope to `src`, `lib`, or a file. Express/NestJS endpoint tasks start with `routes`. For handlers use `symbols app.render --lang javascript --file-pattern lib/application.js --limit 10`, then `symbols/defs --include-code`. | CommonJS assignments like `exports.foo = function`, `app.handle = function`, dynamic router dispatch, or 0-result `defs`/`refs` appear; use `rg` on the exact property/call. |
| Python | FastAPI/Django endpoint tasks start with `routes`. Otherwise use `symbols`/`defs` with `--lang python --dir <pkg>` to locate functions/classes, then shallow `call-hierarchy` only as a lead. | You need precise refs, decorators, monkey patches, dynamic imports, or runtime framework wiring. |
| Java/Kotlin | Spring endpoint tasks start with `routes --framework spring`. Scope production code with `--dir src/main/java` or `--dir src/main/kotlin`; use `--file-pattern` for overloaded or common method names. | The index is stale/missing, annotation-generated behavior matters, or Java/Kotlin cross-language calls are incomplete. |
| Swift | Vapor endpoint tasks start with `routes --framework vapor`. Scope to `Sources/<Target>` or a specific file before `defs`/`symbols` and shallow call queries. | SourceKit or build settings are missing, or protocol/extension dispatch decides behavior. |
| Ruby | Rails endpoint tasks start with `routes --framework rails`. Scope to `app/controllers`, `app/models`, or a specific file before `defs`/`symbols`. | Rails metaprogramming, dynamic dispatch, or Bundler/Sorbet setup hides the target. |

## Broad Query Ban

Do not query these names bare: `run`, `handle`, `render`, `new`, `parse`, `Config`, `Router`, `Description`.

First narrow with `routes`, `--dir`, `--file-pattern`, or a small `symbols`
query that identifies the concrete file.

## Boundaries

- `routes` returns framework route candidates; it does not prove final runtime
  routing behavior.
- `refs` is `precise_fact` only with a fresh SCIP index. If it reports missing
  or stale precise index, use `rg` or `index doctor`.
- `defs` and `symbols` may fall back to tree-sitter `parser_fact` when SCIP is
  unavailable.
- `calls`, `callers`, and `call-hierarchy` are `inferred_candidate` navigation
  leads.
- Verify exact source ranges with the host read/editor before editing or making
  a strong claim.

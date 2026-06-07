# SCIP INDEXER KNOWLEDGE BASE

## OVERVIEW

`scip-indexer/` is a separate Go module that generates SCIP-like JSON for Go projects; Rust invokes it through `src/scip_indexer.rs`.

## STRUCTURE

```text
scip-indexer/
|-- main.go  # go/packages + go/types indexer
|-- go.mod   # module and Go version
`-- go.sum   # locked Go dependencies
```

## WHERE TO LOOK

| Task | Location | Notes |
| --- | --- | --- |
| CLI and output format | `main.go` | `go run main.go --output <file> <project_root>`. |
| Go dependencies | `go.mod`, `go.sum` | Separate from Rust Cargo dependencies. |
| Rust invocation/import | `../src/scip_indexer.rs`, `../src/scip_index.rs` | Shell-out and import behavior. |
| Output consumers | `../src/scip/store.rs`, `../tests/cli.rs` | Precise defs/refs/symbols coverage. |

## CONVENTIONS

- Keep output JSON compatible with the Rust import path: tool info, documents, occurrences, symbols, relative paths, language, ranges, and symbol roles.
- Ranges are zero-based SCIP-style arrays: `[startLine, startCol, endLine, endCol]`.
- Only project-root-relative paths should be emitted.
- Go package load failures may warn, but usable packages should still produce output when possible.
- This directory uses Go formatting and tooling, not Cargo.

## ANTI-PATTERNS

- Do not write output outside the requested `--output` path.
- Do not emit absolute source paths into the index payload.
- Do not change symbol role integers without updating Rust import and tests.
- Do not assume exported-only symbols are enough for every future query; if that policy changes, update Rust consumers and contract tests together.

## VERIFY

```bash
go test ./...
go run ./main.go --output /tmp/codetrail-go-index.scip.json .
```

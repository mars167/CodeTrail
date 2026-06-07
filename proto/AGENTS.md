# PROTO KNOWLEDGE BASE

## OVERVIEW

`proto/` contains the SCIP protobuf schema consumed by `build.rs` and exposed to Rust through `src/scip_proto.rs`.

## WHERE TO LOOK

| Task | Location | Notes |
| --- | --- | --- |
| Schema source | `scip.proto` | Authoritative protobuf definitions. |
| Rust codegen | `../build.rs` | Runs `prost_build::compile_protos`. |
| Generated include wrapper | `../src/scip_proto.rs` | Includes generated Rust from `OUT_DIR`. |
| SCIP parser/store | `../src/scip/`, `../src/scip_index.rs` | Consumers of generated types. |
| Native import tests | `../tests/cli.rs`, `../tests/semantic_facts.rs` | Exercise occurrence and symbol contracts. |

## CONVENTIONS

- Treat `scip.proto` as compatibility-sensitive. Field numbers, enum values, and deprecation markers are part of the wire contract.
- Prefer adding fields over repurposing existing ones.
- Keep deprecated SCIP enum values intact unless all consumers and fixtures prove removal is safe.
- After schema changes, verify generated Rust consumers compile and update parser/store tests.

## ANTI-PATTERNS

- Do not hand-edit generated Rust output.
- Do not reuse removed field numbers or enum values.
- Do not change ranges, symbol roles, or language enum semantics without checking precise defs/refs/symbols behavior.

## VERIFY

```bash
cargo build
cargo test --test cli native_scip
cargo test --test semantic_facts
```

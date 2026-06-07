# TESTS KNOWLEDGE BASE

## OVERVIEW

`tests/` pins public behavior: CLI output contracts, reliability labels, freshness behavior, project graph ownership, config facts, semantic facts, and provider scheduling.

## STRUCTURE

```text
tests/
|-- cli.rs                 # large black-box CLI contract suite
|-- project_graph.rs       # root discovery and dependency/config edges
|-- config_facts.rs        # config extraction, secret masking, parse fallback
|-- semantic_facts.rs      # fact model, SCIP conversion, reliability buckets
|-- semantic_provider.rs   # provider scheduler, budgets, partial/deferred work
`-- cli-integration/       # shell/Python smoke harness against fixture repos
```

## WHERE TO LOOK

| Task | Location | Notes |
| --- | --- | --- |
| Public JSON/text contract | `cli.rs` | External behavior is tested here. |
| Cursor, saved query, broad guard | `cli.rs` | Keep scope/snapshot rejection behavior intact. |
| Index freshness and remote behavior | `cli.rs` | Tests cover stale, dirty, remote verified/unverified states. |
| Project graph changes | `project_graph.rs` | Root IDs and edge schemas are asserted. |
| Config extraction | `config_facts.rs` | Includes secret masking and malformed config fallback. |
| Semantic model/provider work | `semantic_facts.rs`, `semantic_provider.rs` | Provider proof and precision boundaries. |

## CONVENTIONS

- Prefer adding focused regression tests near the behavior being changed; `tests/cli.rs` is acceptable for user-visible command contracts.
- Keep machine contract assertions stable: error/warning `code`, `severity`, and `category` matter more than wording.
- Tests use temporary repositories and command execution; avoid dependence on the developer's checkout state except where explicitly testing git state.
- When changing output shape, update docs and tests together.

## ANTI-PATTERNS

- Do not weaken broad-query, freshness, remote, or reliability assertions to fit an implementation shortcut.
- Do not remove CLI contract coverage to make tests fast.
- Do not assert dynamic details such as absolute temp paths when stable fields are available.

## VERIFY

```bash
cargo test --test cli
cargo test --test project_graph
cargo test --test config_facts
cargo test --test semantic_facts
cargo test --test semantic_provider
```

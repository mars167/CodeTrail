# CONFIG FACTS KNOWLEDGE BASE

## OVERVIEW

`src/config_facts/` extracts source facts and dependency edges from config-like files without leaking secrets or promoting parse fallbacks to semantic proof.

## WHERE TO LOOK

| Task | Location | Notes |
| --- | --- | --- |
| Extractors and edge mapping | `mod.rs` | JSON, YAML, TOML, INI, shell, Makefile, Docker, Kubernetes handling. |
| Project ownership model | `../project_graph.rs` | Root ownership and config/dependency edge semantics. |
| Output caveats | `../output.rs`, `../caveat_contract.rs` | Public caveat shape and stable warning codes. |
| Tests | `../../tests/config_facts.rs` | Secret masking, malformed parse fallback, edge caveats. |

## CONVENTIONS

- Secret-like values must be masked in public output and tests should prove the raw value is absent.
- Malformed structured config should degrade to a source-fact fallback with a machine-readable caveat, not panic.
- Large or unresolved config relationships should emit caveats rather than fabricating precise ownership.
- Keep config facts separate from semantic provider facts; config-derived evidence is not SCIP precision.
- When adding a parser, add failure-path tests as well as happy-path extraction tests.

## ANTI-PATTERNS

- Do not leak tokens, passwords, keys, or secret-looking scalar values in JSON, text, or debug helpers.
- Do not silently drop malformed config when a fallback source fact can be returned.
- Do not make config edges imply ownership when `project_graph.rs` would treat the file as shared or unresolved.
- Do not expand the monolith further without considering a local split by format or edge type.

## VERIFY

```bash
cargo test --test config_facts
cargo test --test project_graph
```

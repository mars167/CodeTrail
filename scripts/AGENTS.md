# SCRIPTS KNOWLEDGE BASE

## OVERVIEW

`scripts/` is the maintained operational surface for local and CI quality gates, benchmark comparison, installer smoke tests, fixture prep, and release asset upload.

## WHERE TO LOOK

| Task | Location | Notes |
| --- | --- | --- |
| Unified gate | `quality-gate.sh` | `pr`, `main`, `bench`, `quick`, `cli`, `full`. |
| Benchmarks | `bench.sh`, `baseline_values/` | Compare and save baseline values. |
| Installer checks | `test-installers.sh` | Unix and PowerShell installer smoke behavior. |
| Fixture setup | `prepare-ruoyi-fixture.sh` | RuoYi fixture prep for smoke/bench flows. |
| Release sync | `upload-gitea-release-assets.sh` | Direct Gitea release API upload. |
| SWE-bench analysis | `analyze_swebench.py` | Python analysis helper. |

## CONVENTIONS

- Keep shell scripts `set -euo pipefail` unless a step intentionally handles failure.
- `quality-gate.sh pr` is the pre-merge gate: fmt, diff whitespace, installer smoke, locked all-target tests.
- `quality-gate.sh main` adds release build and RuoYi smoke when `TEST_REPO` exists or is required.
- `quality-gate.sh bench` depends on `hyperfine`, `jq`, `bc`, a release binary, and `scripts/baseline_values/`.
- `scripts/baseline_values/` is checked-in benchmark data. `scripts/bench.sh save-baseline` rewrites it deliberately.
- Generated benchmark output belongs in `scripts/bench_results/`; do not treat that directory as source.

## ANTI-PATTERNS

- Do not add a second CI entrypoint that bypasses `quality-gate.sh`.
- Do not make missing optional fixture repos fail unless `REQUIRE_TEST_REPO=1`.
- Do not update benchmark baselines as incidental churn in unrelated work.
- Do not print machine-readable command output with extra banners when downstream scripts parse it.

## VERIFY

```bash
scripts/quality-gate.sh pr
scripts/quality-gate.sh main
scripts/quality-gate.sh bench
```

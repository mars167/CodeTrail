#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_TEST_REPO="$ROOT/../RuoYi"
TEST_REPO="${TEST_REPO:-$DEFAULT_TEST_REPO}"
DEFAULT_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
CS_BIN="${CS_BIN:-$DEFAULT_TARGET_DIR/release/codetrail}"
CARGO_BIN="${CARGO_BIN:-}"

PASS=0
FAIL=0
SKIP=0
SUMMARY_PRINTED=0

usage() {
  cat <<'USAGE'
Usage: scripts/quality-gate.sh {pr|main|bench|quick|cli|full}

Commands:
  pr     Run required PR gates: fmt, diff whitespace, all target tests.
  main   Run PR gates plus release build and required RuoYi smoke.
  bench  Run performance regression checks via scripts/bench.sh compare.
  quick  Compatibility alias for pr.
  cli    Compatibility alias for main.
  full   Run main and bench in sequence.

Environment:
  TEST_REPO  Fixture repository path for CLI smoke and benchmarks.
  REQUIRE_TEST_REPO
             When set to 1, missing TEST_REPO fails smoke/bench gates.
  CS_BIN     codetrail binary path. Defaults to
             $CARGO_TARGET_DIR/release/codetrail when CARGO_TARGET_DIR is set,
             otherwise target/release/codetrail.
  CARGO_BIN  Cargo binary path. Defaults to `rustup which cargo` when available.
USAGE
}

note() {
  printf '\n== %s ==\n' "$1"
}

pass() {
  PASS=$((PASS + 1))
  printf '[PASS] %s\n' "$1"
}

fail() {
  FAIL=$((FAIL + 1))
  printf '[FAIL] %s\n' "$1"
}

skip() {
  SKIP=$((SKIP + 1))
  printf '[SKIP] %s\n' "$1"
}

run_step() {
  local label="$1"
  shift
  printf '%s\n' "-> $label"
  if "$@"; then
    pass "$label"
  else
    fail "$label"
    return 1
  fi
}

require_tool() {
  local tool="$1"
  if ! command -v "$tool" >/dev/null 2>&1; then
    fail "required tool missing: $tool"
    return 1
  fi
}

prepend_path_entry() {
  local entry="$1"
  case ":$PATH:" in
    *":$entry:"*) ;;
    *)
      PATH="$entry:$PATH"
      export PATH
      ;;
  esac
}

configure_cargo_bin() {
  if [[ -n "$CARGO_BIN" ]]; then
    prepend_path_entry "$(dirname "$CARGO_BIN")"
    return 0
  fi

  if command -v rustup >/dev/null 2>&1; then
    local resolved
    if resolved="$(rustup which cargo 2>/dev/null)"; then
      # Keep cargo, rustc, rustdoc, and cargo subcommands on the same toolchain.
      prepend_path_entry "$(dirname "$resolved")"
      CARGO_BIN="$resolved"
      return 0
    fi
  fi

  if command -v cargo >/dev/null 2>&1; then
    CARGO_BIN="$(command -v cargo)"
    prepend_path_entry "$(dirname "$CARGO_BIN")"
    return 0
  fi

  return 1
}

run_cargo() {
  if [[ -z "$CARGO_BIN" ]]; then
    configure_cargo_bin || {
      fail "required tool missing: cargo"
      return 1
    }
  else
    prepend_path_entry "$(dirname "$CARGO_BIN")"
  fi
  "$CARGO_BIN" "$@"
}

run_codetrail_json() {
  "$CS_BIN" --output json --path "$TEST_REPO" "$@"
}

assert_codetrail() {
  local label="$1"
  local filter="$2"
  shift 2

  local output
  if ! output="$(run_codetrail_json "$@")"; then
    fail "$label"
    return 1
  fi

  if jq -e "$filter" >/dev/null <<<"$output"; then
    pass "$label"
  else
    fail "$label"
    return 1
  fi
}

run_pr() {
  note "PR quality gate"
  cd "$ROOT"
  run_step "cargo fmt --check" run_cargo fmt --check
  run_step "git diff --check" git diff --check
  run_step "installer smoke tests" "$ROOT/scripts/test-installers.sh"
  run_step "cargo test --all-targets --locked --no-fail-fast" run_cargo test --all-targets --locked --no-fail-fast
}

run_ruoyi_smoke() {
  note "RuoYi L0 smoke"
  if [[ ! -d "$TEST_REPO" ]]; then
    if [[ "${REQUIRE_TEST_REPO:-0}" == "1" ]]; then
      fail "required fixture repo not found: $TEST_REPO"
      return 1
    fi
    skip "non-blocking smoke skipped; fixture repo not found: $TEST_REPO"
    return 0
  fi

  require_tool jq

  assert_codetrail \
    "find RuoYiApplication returns results" \
    '(.results | length >= 1)' \
    find RuoYiApplication

  assert_codetrail \
    "grep selectUserBy regex returns results" \
    '(.results | length >= 3)' \
    grep 'selectUserBy\w+'

  assert_codetrail \
    "glob controller files returns results" \
    '(.results | length >= 10)' \
    glob '**/*Controller.java'

  assert_codetrail \
    "defs include-code returns definition source excerpt" \
    '(.results | length >= 1) and .results[0].path == "ruoyi-admin/src/main/java/com/ruoyi/RuoYiApplication.java" and .results[0].role == "definition" and .results[0].source.path == "ruoyi-admin/src/main/java/com/ruoyi/RuoYiApplication.java" and (.results[0].source.content | contains("SpringApplication.run(RuoYiApplication.class, args);")) and (.results[0].source.truncated == false)' \
    defs RuoYiApplication --include-code

  assert_codetrail \
    "refs ShiroUtils returns source references" \
    '(.results | length >= 5)' \
    refs ShiroUtils

  assert_codetrail \
    "status returns workspace snapshot" \
    '(.results | length == 1) and (.results[0].snapshot_id | type == "string") and (.results[0].dirty | type == "boolean")' \
    status
}

run_main() {
  note "main quality gate"
  cd "$ROOT"
  run_pr
  run_step "cargo build --release --locked" run_cargo build --release --locked --bin codetrail
  run_ruoyi_smoke
}

run_bench() {
  note "benchmark quality gate"
  cd "$ROOT"
  require_tool hyperfine
  require_tool jq
  require_tool bc
  # Reuse release binary if already built (e.g. from 'full' gate)
  if [[ ! -x "$CS_BIN" ]]; then
    run_step "cargo build --release --locked" run_cargo build --release --locked --bin codetrail
  fi
  if [[ ! -d "$TEST_REPO" ]]; then
    if [[ "${REQUIRE_TEST_REPO:-0}" == "1" ]]; then
      fail "required benchmark fixture repo not found: $TEST_REPO"
      return 1
    fi
    skip "benchmark fixture repo not found: $TEST_REPO"
    return 0
  fi
  if [[ ! -d "$ROOT/scripts/baseline_values" ]]; then
    fail "baseline directory missing: scripts/baseline_values"
    return 1
  fi
  run_step "scripts/bench.sh compare" env CS_BIN="$CS_BIN" TEST_REPO="$TEST_REPO" "$ROOT/scripts/bench.sh" compare
}

summary() {
  SUMMARY_PRINTED=1
  printf '\n== quality gate summary ==\n'
  printf 'pass=%s fail=%s skip=%s\n' "$PASS" "$FAIL" "$SKIP"
  [[ "$FAIL" -eq 0 ]]
}

finish() {
  local status=$?
  if [[ "$SUMMARY_PRINTED" -eq 0 ]]; then
    summary || true
  fi
  exit "$status"
}

trap finish EXIT

main() {
  local command="${1:-}"
  case "$command" in
    pr)
      run_pr
      ;;
    main)
      run_main
      ;;
    quick)
      run_pr
      ;;
    cli)
      run_main
      ;;
    bench)
      run_bench
      ;;
    full)
      run_main
      run_bench
      ;;
    -h|--help|help)
      usage
      return 0
      ;;
    *)
      usage
      return 2
      ;;
  esac
  summary
}

main "$@"

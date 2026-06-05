#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

bash -n "$repo_root/install.sh"
sh "$repo_root/install.sh" --help >/dev/null

assert_asset() {
  os="$1"
  arch="$2"
  expected="$3"
  output="$(
    CODE_SEARCH_OS="$os" \
    CODE_SEARCH_ARCH="$arch" \
    sh "$repo_root/install.sh" --dry-run --version v0.1.3
  )"

  if ! printf '%s\n' "$output" | grep -F "asset=$expected" >/dev/null; then
    echo "Expected asset=$expected for $os/$arch, got:" >&2
    printf '%s\n' "$output" >&2
    exit 1
  fi
}

assert_asset Darwin x86_64 code-search-darwin-amd64.tar.gz
assert_asset Darwin arm64 code-search-darwin-arm64.tar.gz
assert_asset Linux x86_64 code-search-linux-amd64.tar.gz

if command -v pwsh >/dev/null 2>&1; then
  pwsh -NoLogo -NoProfile -NonInteractive -Command "\$ErrorActionPreference = 'Stop'; \$null = [System.Management.Automation.PSParser]::Tokenize((Get-Content -Raw '$repo_root/install.ps1'), [ref]\$null)"
  CODE_SEARCH_ARCH=X64 CODE_SEARCH_DRY_RUN=1 pwsh -NoLogo -NoProfile -NonInteractive -File "$repo_root/install.ps1" | grep -F 'asset=code-search-windows-amd64.exe.zip' >/dev/null
  CODE_SEARCH_ARCH=Arm64 CODE_SEARCH_DRY_RUN=1 pwsh -NoLogo -NoProfile -NonInteractive -File "$repo_root/install.ps1" | grep -F 'asset=code-search-windows-arm64.exe.zip' >/dev/null
fi

echo "Installer smoke tests passed."

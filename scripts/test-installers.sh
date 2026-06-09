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
    CODETRAIL_OS="$os" \
    CODETRAIL_ARCH="$arch" \
    sh "$repo_root/install.sh" --dry-run --version v0.1.4
  )"

  if ! printf '%s\n' "$output" | grep -F "asset=$expected" >/dev/null; then
    echo "Expected asset=$expected for $os/$arch, got:" >&2
    printf '%s\n' "$output" >&2
    exit 1
  fi
}

assert_asset Darwin x86_64 codetrail-darwin-amd64.tar.gz
assert_asset Darwin arm64 codetrail-darwin-arm64.tar.gz
assert_asset Linux x86_64 codetrail-linux-amd64.tar.gz

if command -v pwsh >/dev/null 2>&1; then
  pwsh -NoLogo -NoProfile -NonInteractive -Command "\$ErrorActionPreference = 'Stop'; \$null = [System.Management.Automation.PSParser]::Tokenize((Get-Content -Raw '$repo_root/install.ps1'), [ref]\$null)"
  CODETRAIL_ARCH=X64 CODETRAIL_DRY_RUN=1 pwsh -NoLogo -NoProfile -NonInteractive -File "$repo_root/install.ps1" | grep -F 'asset=codetrail-windows-amd64.exe.zip' >/dev/null
  CODETRAIL_ARCH=Arm64 CODETRAIL_DRY_RUN=1 pwsh -NoLogo -NoProfile -NonInteractive -File "$repo_root/install.ps1" | grep -F 'asset=codetrail-windows-arm64.exe.zip' >/dev/null
elif [[ "${REQUIRE_PWSH:-0}" == "1" ]]; then
  echo "PowerShell installer smoke requested but pwsh is missing" >&2
  exit 1
else
  echo "PowerShell installer smoke skipped: pwsh not found" >&2
fi

echo "Installer smoke tests passed."

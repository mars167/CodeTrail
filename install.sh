#!/bin/sh
set -eu

repo="${CODETRAIL_REPO:-mars167/CodeTrail}"
version="${CODETRAIL_VERSION:-latest}"
install_dir="${CODETRAIL_INSTALL_DIR:-$HOME/.local/bin}"
dry_run="${CODETRAIL_DRY_RUN:-0}"

usage() {
  cat <<'USAGE'
Install codetrail from GitHub release assets.

Usage:
  install.sh [--version <tag>] [--repo <owner/repo>] [--install-dir <dir>] [--dry-run]

Environment:
  CODETRAIL_VERSION       Release tag to install, defaults to latest.
  CODETRAIL_REPO          GitHub repository, defaults to mars167/CodeTrail.
  CODETRAIL_INSTALL_DIR   Destination directory, defaults to $HOME/.local/bin.
  CODETRAIL_OS            Override detected OS for tests.
  CODETRAIL_ARCH          Override detected architecture for tests.
  CODETRAIL_DRY_RUN       Set to 1 to print selected asset without downloading.
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      [ "$#" -ge 2 ] || { echo "Missing value for --version" >&2; exit 1; }
      version="$2"
      shift 2
      ;;
    --repo)
      [ "$#" -ge 2 ] || { echo "Missing value for --repo" >&2; exit 1; }
      repo="$2"
      shift 2
      ;;
    --install-dir)
      [ "$#" -ge 2 ] || { echo "Missing value for --install-dir" >&2; exit 1; }
      install_dir="$2"
      shift 2
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

download() {
  url="$1"
  output="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$output"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$output" "$url"
  else
    echo "Missing required command: curl or wget" >&2
    exit 1
  fi
}

checksum() {
  file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print $1}'
  else
    echo "Missing required command: sha256sum or shasum" >&2
    exit 1
  fi
}

os_name="${CODETRAIL_OS:-$(uname -s)}"
arch_name="${CODETRAIL_ARCH:-$(uname -m)}"

case "$os_name" in
  Darwin) os="darwin" ;;
  Linux) os="linux" ;;
  *)
    echo "Unsupported OS: $os_name" >&2
    exit 1
    ;;
esac

case "$arch_name" in
  x86_64|amd64) arch="amd64" ;;
  arm64|aarch64) arch="arm64" ;;
  *)
    echo "Unsupported architecture: $arch_name" >&2
    exit 1
    ;;
esac

case "${os}-${arch}" in
  darwin-amd64|darwin-arm64|linux-amd64)
    asset="codetrail-${os}-${arch}.tar.gz"
    ;;
  linux-arm64)
    echo "No Linux ARM64 release asset is currently published." >&2
    exit 1
    ;;
  *)
    echo "Unsupported platform: ${os}-${arch}" >&2
    exit 1
    ;;
esac

if [ "$version" = "latest" ]; then
  base_url="https://github.com/${repo}/releases/latest/download"
else
  base_url="https://github.com/${repo}/releases/download/${version}"
fi

if [ "$dry_run" = "1" ]; then
  printf 'repo=%s\nversion=%s\nasset=%s\ninstall_dir=%s\nurl=%s/%s\n' \
    "$repo" "$version" "$asset" "$install_dir" "$base_url" "$asset"
  exit 0
fi

need_cmd awk
need_cmd tar
need_cmd mktemp

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT INT TERM

asset_path="$tmp_dir/$asset"
checksums_path="$tmp_dir/SHA256SUMS"

echo "Downloading ${asset}..."
download "${base_url}/${asset}" "$asset_path"
download "${base_url}/SHA256SUMS" "$checksums_path"

expected="$(awk -v file="$asset" '$2 == file { print $1 }' "$checksums_path")"
if [ -z "$expected" ]; then
  echo "Checksum for ${asset} was not found in SHA256SUMS." >&2
  exit 1
fi

actual="$(checksum "$asset_path")"
if [ "$expected" != "$actual" ]; then
  echo "Checksum mismatch for ${asset}." >&2
  echo "expected: $expected" >&2
  echo "actual:   $actual" >&2
  exit 1
fi

extract_dir="$tmp_dir/extract"
mkdir -p "$extract_dir"
tar -xzf "$asset_path" -C "$extract_dir"

if [ ! -f "$extract_dir/codetrail" ]; then
  echo "Release archive did not contain codetrail." >&2
  exit 1
fi

mkdir -p "$install_dir"
cp "$extract_dir/codetrail" "$install_dir/codetrail"
chmod +x "$install_dir/codetrail"

echo "Installed codetrail to ${install_dir}/codetrail"

# Refresh shell command hash so a newly installed binary is visible immediately.
hash -r 2>/dev/null || true

if command -v codetrail >/dev/null 2>&1; then
  echo "codetrail is ready to use."
  exit 0
fi

install_dir_in_path=false
case ":$PATH:" in
  *":$install_dir:"*) install_dir_in_path=true ;;
esac

if $install_dir_in_path; then
  echo "Run 'hash -r' or open a new terminal for codetrail to be available."
  exit 0
fi

shell_name="$(basename "${SHELL:-/bin/sh}")"
rc_file=""
case "$shell_name" in
  zsh)  rc_file="${ZDOTDIR:-$HOME}/.zshrc" ;;
  bash)
    if [ -f "$HOME/.bash_profile" ]; then
      rc_file="$HOME/.bash_profile"
    elif [ -f "$HOME/.bashrc" ]; then
      rc_file="$HOME/.bashrc"
    else
      rc_file="$HOME/.bashrc"
    fi
    ;;
  *)    rc_file="$HOME/.profile" ;;
esac

if grep -qF "export PATH=\"$install_dir" "$rc_file" 2>/dev/null; then
  echo "PATH entry already exists in $rc_file."
else
  printf '\n# Added by codetrail installer\n' >> "$rc_file"
  printf 'export PATH="%s:$PATH"\n' "$install_dir" >> "$rc_file"
  echo "Added $install_dir to PATH in $rc_file"
fi

echo "Run 'source $rc_file' or open a new terminal to use codetrail."

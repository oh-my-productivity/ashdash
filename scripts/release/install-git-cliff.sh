#!/usr/bin/env bash
# Installs the pinned git-cliff (scripts/release/tools.env) into a directory and verifies its SHA-256 first.
#
# Usage: install-git-cliff.sh [DEST_DIR]        (default: $RUNNER_TEMP/tools or ~/.cache/ashdash-release-tools)
# Prints the directory that holds `git-cliff`; with GITHUB_PATH set (GitHub Actions) it also adds it to the PATH.
# Environment: GIT_CLIFF_ARCHIVE=<file> uses a local archive instead of downloading it (the checksum is still verified).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/release/tools.env
. "$here/tools.env"

dest="${1:-${RUNNER_TEMP:+$RUNNER_TEMP/tools}}"
dest="${dest:-$HOME/.cache/ashdash-release-tools}"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)
    target="aarch64-apple-darwin"
    expected="$GIT_CLIFF_SHA256_DARWIN_ARM64"
    ;;
  Linux-x86_64)
    target="x86_64-unknown-linux-musl"
    expected="$GIT_CLIFF_SHA256_LINUX_X86_64"
    ;;
  *)
    echo "error: no pinned git-cliff for $(uname -s)-$(uname -m); install git-cliff $GIT_CLIFF_VERSION yourself" >&2
    exit 69
    ;;
esac

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

name="git-cliff-$GIT_CLIFF_VERSION-$target" # the archive; its top-level directory is git-cliff-<version>
top="git-cliff-$GIT_CLIFF_VERSION"
if [ -x "$dest/git-cliff" ] && [ "$("$dest/git-cliff" --version 2>/dev/null)" = "git-cliff $GIT_CLIFF_VERSION" ]; then
  echo "git-cliff $GIT_CLIFF_VERSION already installed in $dest" >&2
else
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  archive="${GIT_CLIFF_ARCHIVE:-}"
  if [ -z "$archive" ]; then
    archive="$work/$name.tar.gz"
    url="https://github.com/orhun/git-cliff/releases/download/v$GIT_CLIFF_VERSION/$name.tar.gz"
    echo "downloading $url" >&2
    curl --fail --silent --show-error --location --retry 3 --output "$archive" "$url"
  fi
  actual="$(sha256_of "$archive")"
  if [ "$actual" != "$expected" ]; then
    echo "error: SHA-256 mismatch for $name.tar.gz" >&2
    echo "  expected $expected" >&2
    echo "  actual   $actual" >&2
    exit 1
  fi
  tar -xzf "$archive" -C "$work" "$top/git-cliff"
  mkdir -p "$dest"
  install -m 755 "$work/$top/git-cliff" "$dest/git-cliff"
  echo "installed git-cliff $GIT_CLIFF_VERSION (sha256 verified) in $dest" >&2
fi

if [ -n "${GITHUB_PATH:-}" ]; then
  echo "$dest" >>"$GITHUB_PATH"
fi
printf '%s\n' "$dest"

#!/usr/bin/env bash
# The gate in front of the deploy key: checks that a commit built by an earlier job is exactly a release commit.
#
# Usage: verify-release-commit.sh BASE COMMIT VERSION
#
# Passes only if COMMIT has BASE as its only parent, its subject is "chore(release): vVERSION", it changes nothing but
# the version files (Cargo.toml, crates/*/Cargo.toml, Cargo.lock, CHANGELOG.md), Cargo.lock only in `version = ` lines,
# and both crates are at VERSION in COMMIT. It reads git objects only and runs nothing from the commit.
# Exit status: 0 fine; 64 wrong arguments; 65 the commit is not a release commit.
set -euo pipefail

SELF="verify-release-commit.sh"
# shellcheck source=scripts/release/lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

[ $# -eq 3 ] || die "$EX_USAGE" "usage: verify-release-commit.sh BASE COMMIT VERSION"
base="$1" commit="$2" version="${3#v}"
is_version "$version" || die "$EX_USAGE" "'$version' is not a release version"

base="$(git rev-parse --verify -q "$base^{commit}")" || die "$EX_STATE" "BASE is not a commit"
commit="$(git rev-parse --verify -q "$commit^{commit}")" || die "$EX_STATE" "COMMIT is not a commit"

parents="$(git rev-list --parents -n 1 "$commit" | cut -d' ' -f2-)"
[ "$parents" = "$base" ] || die "$EX_STATE" "the release commit must have exactly one parent, $base (it has: ${parents:-none})"

subject="$(git show -s --format=%s "$commit")"
[ "$subject" = "chore(release): v$version" ] || die "$EX_STATE" "unexpected subject: '$subject'"

while IFS= read -r path; do
  [ -n "$path" ] || continue
  case "$path" in
    Cargo.toml | Cargo.lock | CHANGELOG.md | crates/*/Cargo.toml) ;;
    *) die "$EX_STATE" "the release commit changes $path, which a release never touches" ;;
  esac
done < <(git diff --name-only "$base" "$commit")
[ -n "$(git diff --name-only "$base" "$commit")" ] || die "$EX_STATE" "the release commit changes nothing"

bad="$(git diff -U0 "$base" "$commit" -- Cargo.lock | grep -E '^[+-]' | grep -vE '^(\+\+\+|---) ' | grep -vE '^[+-]version = "' || true)"
[ -z "$bad" ] || die "$EX_STATE" "Cargo.lock changes more than version lines"

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
for manifest in crates/app/Cargo.toml crates/core/Cargo.toml; do
  git show "$commit:$manifest" >"$tmp" 2>/dev/null || die "$EX_STATE" "$manifest does not exist in the release commit"
  found="$(manifest_version "$tmp")"
  [ "$found" = "$version" ] || die "$EX_STATE" "$manifest says '$found' in the release commit, not $version"
done
git show "$commit:CHANGELOG.md" 2>/dev/null | grep -qF "## [$version] - " || die "$EX_STATE" "CHANGELOG.md has no section for $version in the release commit"

echo "release commit ${commit:0:12} for $version is well formed (parent ${base:0:12})"

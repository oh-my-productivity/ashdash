#!/usr/bin/env bash
# Render the Homebrew cask for a release. Prints the cask to stdout (or writes --output).
#
# Usage: render-cask.sh --dmg Ashdash-0.1.0.dmg [--repo owner/name] [--output file.rb]
#        render-cask.sh --version 0.1.0 --sha256 <hex> [--repo owner/name]
# The DMG's .sha256 file (written by make-dmg.sh) is used when --dmg is given and no --sha256 is.
# The repository defaults to $GITHUB_REPOSITORY, else the placeholder oh-my-productivity/ashdash.
set -euo pipefail
# shellcheck source=../lib.sh
. "$(dirname "$0")/../lib.sh"

version="" sha="" dmg="" repo="${GITHUB_REPOSITORY:-oh-my-productivity/ashdash}" output=""
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version="${2:?--version needs a value}"; shift 2 ;;
    --sha256) sha="${2:?--sha256 needs a value}"; shift 2 ;;
    --dmg) dmg="${2:?--dmg needs a value}"; shift 2 ;;
    --repo) repo="${2:?--repo needs a value}"; shift 2 ;;
    --output) output="${2:?--output needs a value}"; shift 2 ;;
    *) die "unknown argument: $1" ;;
  esac
done

NAME="$(brand_const NAME)"
SLUG="$(brand_const SLUG)"

if [ -n "$dmg" ]; then
  [ -f "$dmg" ] || die "dmg not found: $dmg"
  base="$(basename "$dmg")"
  if [ -z "$version" ]; then
    version="${base#"$NAME"-}"
    version="${version%.dmg}"
  fi
  if [ -z "$sha" ]; then
    sha="$(shasum -a 256 "$dmg" | cut -d' ' -f1)"
  fi
fi
[ -n "$version" ] || die "need --version or --dmg"
[ -n "$sha" ] || die "need --sha256 or --dmg"

printf '%s' "$sha" | grep -Eq '^[0-9a-f]{64}$' || die "sha256 must be 64 lowercase hex characters"
printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$' || die "odd version: $version"
printf '%s' "$repo" | grep -Eq '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$' || die "repo must look like owner/name: $repo"

rendered="$(render_template "$PACKAGING_DIR/homebrew/ashdash.rb.template" \
  "SLUG=$SLUG" "NAME=$NAME" "VERSION=$version" "SHA256=$sha" "REPO=$repo")"
if [ -n "$output" ]; then
  printf '%s\n' "$rendered" >"$output"
  log "wrote $output"
else
  printf '%s\n' "$rendered"
fi

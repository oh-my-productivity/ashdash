#!/usr/bin/env bash
# Shared helpers for the packaging scripts. Source it; do not execute it.
# shellcheck shell=bash

PACKAGING_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$PACKAGING_DIR/.." && pwd)"
export PACKAGING_DIR REPO_ROOT

log() { printf '== %s\n' "$*" >&2; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || die "required tool not found: $1"
}

# brand_const NAME -> value of `pub const NAME: &str = "value";` in brand.rs.
# brand.rs is the single source of truth for names and the bundle id.
brand_const() {
  local value
  value="$(sed -n "s/^pub const $1: &str = \"\\(.*\\)\";.*/\\1/p" "$REPO_ROOT/crates/core/src/brand.rs" | head -n 1)"
  [ -n "$value" ] || die "brand.rs has no constant $1"
  printf '%s' "$value"
}

# app_version -> the version of the `ashdash` crate (first `version = "..."`).
app_version() {
  local value
  value="$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$REPO_ROOT/crates/app/Cargo.toml" | head -n 1)"
  [ -n "$value" ] || die "cannot read the version from crates/app/Cargo.toml"
  printf '%s' "$value"
}

# build_number -> CFBundleVersion. BUILD_NUMBER wins; else the commit count; else the version.
build_number() {
  if [ -n "${BUILD_NUMBER:-}" ]; then
    printf '%s' "$BUILD_NUMBER"
  elif n="$(git -C "$REPO_ROOT" rev-list --count HEAD 2>/dev/null)" && [ -n "$n" ]; then
    printf '%s' "$n"
  else
    app_version
  fi
}

# is_adhoc_identity IDENTITY -> success when the identity means "ad-hoc".
is_adhoc_identity() {
  [ -z "$1" ] || [ "$1" = "-" ]
}

# signing_identity -> CODESIGN_IDENTITY, defaulting to ad-hoc ("-").
signing_identity() {
  printf '%s' "${CODESIGN_IDENTITY:--}"
}

# render_template FILE KEY=VALUE... -> stdout. Replaces every @KEY@; fails on leftovers.
render_template() {
  local file="$1"
  shift
  local content pair key value
  content="$(cat "$file")"
  for pair in "$@"; do
    key="${pair%%=*}"
    value="${pair#*=}"
    content="${content//@${key}@/$value}"
  done
  if printf '%s' "$content" | grep -q '@[A-Z][A-Z0-9_]*@'; then
    die "unreplaced placeholder in $file: $(printf '%s' "$content" | grep -o '@[A-Z][A-Z0-9_]*@' | sort -u | tr '\n' ' ')"
  fi
  printf '%s\n' "$content"
}

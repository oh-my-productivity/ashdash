# Helpers for the release script tests: synthetic repositories and tiny assertions. Source it. Bash 3.2 compatible.
# shellcheck shell=bash

REL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd "$REL_DIR/../.." && pwd)"

# Every commit made by the tests uses this identity; nothing is read from the user's git configuration.
export GIT_AUTHOR_NAME="Amin Alizadeh" GIT_COMMITTER_NAME="Amin Alizadeh"
export GIT_AUTHOR_EMAIL="64799918+achieveramin@users.noreply.github.com"
export GIT_COMMITTER_EMAIL="64799918+achieveramin@users.noreply.github.com"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1
export CLIFF_RELEASE_DATE="2030-01-02"

TEST_TMP="$(mktemp -d)"
trap 'rm -rf "$TEST_TMP"' EXIT

PASS=0
FAIL=0
FAILED_NAMES=""

pass() { PASS=$((PASS + 1)); printf 'ok   %s\n' "$1"; }
fail() {
  FAIL=$((FAIL + 1))
  FAILED_NAMES="$FAILED_NAMES
  $1"
  printf 'FAIL %s\n' "$1"
  shift
  if [ $# -gt 0 ]; then printf '     %s\n' "$@"; fi
}

# Tool lookup: the tests need the pinned git-cliff. Locally a missing tool skips them; CI sets REQUIRE_GIT_CLIFF=1.
require_cliff_or_skip() {
  # shellcheck source=scripts/release/tools.env
  . "$REL_DIR/tools.env"
  local bin="${GIT_CLIFF:-git-cliff}"
  if ! command -v "$bin" >/dev/null 2>&1 || [ "$("$bin" --version 2>/dev/null)" != "git-cliff $GIT_CLIFF_VERSION" ]; then
    if [ "${REQUIRE_GIT_CLIFF:-}" = 1 ]; then
      echo "git-cliff $GIT_CLIFF_VERSION is required (scripts/release/install-git-cliff.sh)" >&2
      exit 1
    fi
    echo "SKIP: git-cliff $GIT_CLIFF_VERSION not found (run scripts/release/install-git-cliff.sh and put it on PATH)"
    exit 0
  fi
}

finish() {
  echo
  echo "$1: $PASS passed, $FAIL failed"
  if [ "$FAIL" -gt 0 ]; then
    printf 'failed:%s\n' "$FAILED_NAMES"
    exit 1
  fi
}

# --- synthetic repositories ---------------------------------------------------------------------------------------

# new_repo [VERSION] [--lock]: creates a repository with the two manifests at VERSION (default 0.1.0), cliff.toml and
# one commit; changes into it. With --lock a minimal workspace is created that cargo can lock offline.
new_repo() {
  local version="${1:-0.1.0}" lock="${2:-}"
  REPO="$(mktemp -d "$TEST_TMP/repo.XXXXXX")"
  cd "$REPO" || return 1
  git init -q -b main
  mkdir -p crates/app/src crates/core/src
  printf '[workspace]\nmembers = ["crates/core", "crates/app"]\nresolver = "2"\n' >Cargo.toml
  cat >crates/core/Cargo.toml <<EOF
[package]
name = "ashdash-core"
version = "$version"
edition = "2024"
publish = false

[dependencies]
EOF
  cat >crates/app/Cargo.toml <<EOF
[package]
name = "ashdash"
version = "$version"
edition = "2024"
publish = false

[dependencies]
ashdash-core = { version = "$version", path = "../core" }
EOF
  echo 'pub fn core() {}' >crates/core/src/lib.rs
  echo 'fn main() { ashdash_core::core(); }' >crates/app/src/main.rs
  printf 'target/\ndist/\n' >.gitignore
  cp "$REPO_ROOT/cliff.toml" cliff.toml
  printf '# Changelog\n\nAll notable changes to Ashdash are documented here.\n' >CHANGELOG.md
  if [ "$lock" = --lock ]; then
    cargo metadata --offline --format-version 1 >/dev/null 2>&1
  fi
  git add -A
  git commit -q -m "chore: initial import"
}

# c SUBJECT [BODY]: an empty commit (the content does not matter, the message does).
c() {
  if [ $# -ge 2 ]; then
    git commit -q --allow-empty -m "$1" -m "$2"
  else
    git commit -q --allow-empty -m "$1"
  fi
}

# set_version VERSION: rewrites the versions in both manifests (no lock file handling).
set_version() {
  local f
  for f in crates/app/Cargo.toml crates/core/Cargo.toml; do
    sed -e "s/^version = \".*\"/version = \"$1\"/" -e "s/^\\(ashdash-core = { version = \\)\"[^\"]*\"/\\1\"$1\"/" "$f" >"$f.new"
    mv "$f.new" "$f"
  done
}

# release VERSION: what a release leaves behind: manifests at VERSION, the chore(release) commit and the annotated tag.
release() {
  set_version "$1"
  git add -A
  git commit -q --allow-empty -m "chore(release): v$1"
  git tag -a "v$1" -m "v$1"
}

# --- assertions -----------------------------------------------------------------------------------------------------

# run_nv ARGS...: runs next-version.sh in the current repository; sets NV_OUT, NV_ERR and NV_STATUS.
run_nv() {
  local out="$TEST_TMP/nv.out" err="$TEST_TMP/nv.err"
  NV_STATUS=0
  "$REL_DIR/next-version.sh" "$@" >"$out" 2>"$err" || NV_STATUS=$?
  NV_OUT="$(cat "$out")"
  NV_ERR="$(cat "$err")"
}

# expect_version NAME VERSION ARGS...
expect_version() {
  local name="$1" want="$2"
  shift 2
  run_nv "$@"
  if [ "$NV_STATUS" = 0 ] && [ "$NV_OUT" = "$want" ]; then
    pass "$name"
  else
    fail "$name" "wanted version $want, got status $NV_STATUS stdout '$NV_OUT'" "stderr: $NV_ERR"
  fi
}

# expect_status NAME STATUS [ERROR-SUBSTRING] ARGS...  (use -- to separate when the substring is empty)
expect_status() {
  local name="$1" want="$2" needle="$3"
  shift 3
  run_nv "$@"
  if [ "$NV_STATUS" != "$want" ]; then
    fail "$name" "wanted status $want, got $NV_STATUS stdout '$NV_OUT'" "stderr: $NV_ERR"
  elif [ -n "$NV_OUT" ]; then
    fail "$name" "stdout must be empty when no version is printed, got '$NV_OUT'"
  elif [ -n "$needle" ] && ! grep -qiF -- "$needle" <<<"$NV_ERR"; then
    fail "$name" "stderr does not mention '$needle'" "stderr: $NV_ERR"
  else
    pass "$name"
  fi
}

# expect_contains NAME FILE-OR-TEXT NEEDLE
expect_in() {
  if grep -qF -- "$3" <<<"$2"; then pass "$1"; else fail "$1" "missing: $3" "in: $2"; fi
}
expect_not_in() {
  if grep -qF -- "$3" <<<"$2"; then fail "$1" "unexpected: $3" "in: $2"; else pass "$1"; fi
}
expect_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1" "wanted '$3'" "got    '$2'"; fi
}

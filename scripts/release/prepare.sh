#!/usr/bin/env bash
# Prepares the working tree for a release: the version in the manifests and in Cargo.lock, the CHANGELOG.md section
# and the notes of the GitHub release. It changes files only; committing and tagging is the release workflow's job.
#
# Usage: prepare.sh VERSION [--notes-file FILE] [--date YYYY-MM-DD] [--allow-dirty]
#
#   VERSION         X.Y.Z or X.Y.Z-rc.N (what next-version.sh printed)
#   --notes-file F  where to write the release notes (default dist/release-notes.md)
#   --date D        the date of the CHANGELOG section (default: today, UTC; also CLIFF_RELEASE_DATE)
#
# What it touches (nothing else; it refuses to finish if anything else changed):
#   crates/app/Cargo.toml, crates/core/Cargo.toml   the package version, and the `version` of the path dependencies
#                                                    between workspace crates; [workspace.package] in Cargo.toml if present
#   Cargo.lock                                      the version of the workspace packages only, NO `cargo update`
#   CHANGELOG.md                                    the section of this release is generated from the commits since the previous
#                                                    stable tag; "## [Unreleased]" and "## [X.Y.Z] - unreleased" are folded into it,
#                                                    sections of earlier candidates of the same version are replaced
#   dist/release-notes.md                           docs/release-notes/vX.Y.Z.md if it exists (hand-written introduction), then the
#                                                    changes, then the install block docs/release-notes/install.md (only when there is no
#                                                    hand-written introduction)
#
# Afterwards `cargo metadata --locked` proves that the lock file is consistent (add PREPARE_OFFLINE=1 for --offline;
# PREPARE_SKIP_CARGO=1 skips the proof, for previews that must not download the dependencies).
# Environment: CLIFF_CONFIG, GIT_CLIFF, CLIFF_REPO_URL (default: $GITHUB_SERVER_URL/$GITHUB_REPOSITORY, else cliff.toml's).
# Exit status: 0 prepared; 64 wrong arguments; 65 the repository is not in a releasable state; 69 a tool is missing.
set -euo pipefail

SELF="prepare.sh"
# shellcheck source=scripts/release/lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

version="" notes_file="dist/release-notes.md" date="${CLIFF_RELEASE_DATE:-}" allow_dirty=0
while [ $# -gt 0 ]; do
  case "$1" in
    --notes-file)
      [ $# -ge 2 ] || die "$EX_USAGE" "--notes-file needs a path"
      notes_file="$2"
      shift 2
      ;;
    --date)
      [ $# -ge 2 ] || die "$EX_USAGE" "--date needs YYYY-MM-DD"
      date="$2"
      shift 2
      ;;
    --allow-dirty)
      allow_dirty=1
      shift
      ;;
    -h | --help)
      sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    -*) die "$EX_USAGE" "unknown argument: $1 (see --help)" ;;
    *)
      [ -z "$version" ] || die "$EX_USAGE" "only one version can be prepared"
      version="${1#v}"
      shift
      ;;
  esac
done
[ -n "$version" ] || die "$EX_USAGE" "missing VERSION (see --help)"
is_version "$version" || die "$EX_USAGE" "'$version' is not a release version (X.Y.Z or X.Y.Z-rc.N)"
date="${date:-$(date -u +%Y-%m-%d)}"
[[ "$date" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || die "$EX_USAGE" "the date must look like 2030-01-31 (got '$date')"

require_tool git
require_tool jq "used to read cargo's metadata"
if [ "${PREPARE_SKIP_CARGO:-}" != 1 ]; then require_tool cargo; fi
find_git_cliff

root="$(git rev-parse --show-toplevel 2>/dev/null)" || die "$EX_STATE" "not inside a git repository"
cd "$root"
CLIFF_CONFIG="${CLIFF_CONFIG:-$root/cliff.toml}"
[ -f "$CLIFF_CONFIG" ] || die "$EX_STATE" "$CLIFF_CONFIG not found"
[ -f CHANGELOG.md ] || die "$EX_STATE" "CHANGELOG.md not found"
[ -f Cargo.lock ] || die "$EX_STATE" "Cargo.lock not found (the release must not create it)"
if [ "$allow_dirty" = 0 ] && [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  die "$EX_STATE" "the working tree has uncommitted changes to tracked files (commit or stash them, or use --allow-dirty)"
fi
if [ "$(git rev-parse --is-shallow-repository)" = true ]; then
  die "$EX_STATE" "this is a shallow clone: the changelog would miss commits (fetch with full history)"
fi

export CLIFF_RELEASE_DATE="$date"
if [ -z "${CLIFF_REPO_URL:-}" ] && [ -n "${GITHUB_REPOSITORY:-}" ]; then
  CLIFF_REPO_URL="${GITHUB_SERVER_URL:-https://github.com}/$GITHUB_REPOSITORY"
fi
if [ -n "${CLIFF_REPO_URL:-}" ]; then export CLIFF_REPO_URL; fi
repo_url="${CLIFF_REPO_URL:-https://github.com/oh-my-productivity/ashdash}"

repo_versions
[ "$APP_VERSION" = "$CORE_VERSION" ] ||
  die "$EX_STATE" "$APP_MANIFEST ($APP_VERSION) and $CORE_MANIFEST ($CORE_VERSION) have different versions"
last_stable="$(highest_tag stable)"
if git rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
  die "$EX_STATE" "the tag v$version already exists"
fi
case "$(version_cmp "$version" "$APP_VERSION")" in
  1) ;;
  0) [ -z "$last_stable" ] || die "$EX_STATE" "the manifests already say $version" ;;
  *) die "$EX_STATE" "$version is lower than the version in the manifests ($APP_VERSION)" ;;
esac

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# --- 1. manifests ---------------------------------------------------------------------------------------------------

# Names of the workspace packages, from the manifests themselves.
package_name() { awk '/^\[/ { p = ($0 == "[package]") } p && /^name[[:space:]]*=/ { v = $0; sub(/^[^"]*"/, "", v); sub(/".*$/, "", v); print v; exit }' "$1"; }
names="$(package_name "$APP_MANIFEST") $(package_name "$CORE_MANIFEST")"

# rewrite_manifest FILE: the [package] (or [workspace.package]) version, and `name = { version = "..", path = ".." }`
# dependencies on workspace packages. Everything else is copied byte for byte.
rewrite_manifest() {
  awk -v new="$version" -v names="$names" '
    BEGIN { n = split(names, list, " "); for (i = 1; i <= n; i++) ws[list[i]] = 1 }
    /^\[/ { in_pkg = ($0 == "[package]" || $0 == "[workspace.package]"); done_pkg = 0 }
    in_pkg && !done_pkg && /^version[[:space:]]*=[[:space:]]*"/ { sub(/"[^"]*"/, "\"" new "\""); done_pkg = 1 }
    {
      key = $0
      sub(/[[:space:]]*=.*$/, "", key)
      if (key in ws && $0 ~ /\{/ && $0 ~ /path[[:space:]]*=/ && $0 ~ /version[[:space:]]*=[[:space:]]*"/) {
        sub(/version[[:space:]]*=[[:space:]]*"[^"]*"/, "version = \"" new "\"")
      }
      print
    }' "$1" >"$work/manifest.new"
  cat "$work/manifest.new" >"$1"
}

for f in Cargo.toml "$APP_MANIFEST" "$CORE_MANIFEST"; do
  if [ -f "$f" ]; then rewrite_manifest "$f"; fi
done

# --- 2. Cargo.lock: the workspace packages only ---------------------------------------------------------------------

awk -v new="$version" -v names="$names" '
  BEGIN { n = split(names, list, " "); for (i = 1; i <= n; i++) ws[list[i]] = 1; count = 0 }
  function flush(   i, name, has_source) {
    name = ""; has_source = 0
    for (i = 1; i <= count; i++) {
      if (buf[i] ~ /^name = "/) { name = buf[i]; sub(/^name = "/, "", name); sub(/".*$/, "", name) }
      if (buf[i] ~ /^source = /) has_source = 1
    }
    for (i = 1; i <= count; i++) {
      if ((name in ws) && !has_source && buf[i] ~ /^version = "/) buf[i] = "version = \"" new "\""
      print buf[i]
    }
    count = 0
  }
  /^\[\[package\]\]/ { flush(); buf[++count] = $0; in_block = 1; next }
  in_block { buf[++count] = $0; next }
  { print }
  END { flush() }
' Cargo.lock >"$work/Cargo.lock.new"
cat "$work/Cargo.lock.new" >Cargo.lock

# --- 3. the CHANGELOG section ---------------------------------------------------------------------------------------

mkdir "$work/sections"
awk -v dir="$work/sections" '
  BEGIN { out = dir "/header" }
  /^## / { n++; out = sprintf("%s/sec.%03d", dir, n) }
  { print > out }
' CHANGELOG.md
touch "$work/sections/header"

core="$(core_of "$version")"
re_unreleased='^## \[Unreleased\]'
re_draft='^## \[([^]]+)\] - [Uu]nreleased'
re_section='^## \[([^]]+)\]'
drafts=() keep=()
shopt -s nullglob
for sec in "$work"/sections/sec.*; do
  head_line="$(head -n 1 "$sec")"
  if [[ "$head_line" =~ $re_unreleased ]]; then
    drafts+=("$sec")
  elif [[ "$head_line" =~ $re_draft ]] && [ "$(core_of "${BASH_REMATCH[1]}")" = "$core" ]; then
    drafts+=("$sec")
  elif [[ "$head_line" =~ $re_section ]]; then
    sec_version="${BASH_REMATCH[1]}"
    if [ "$sec_version" = "$version" ]; then
      die "$EX_STATE" "CHANGELOG.md already has a section for $version"
    elif is_stable "$version" && [ "$(core_of "$sec_version")" = "$core" ] && [ "$sec_version" != "$core" ]; then
      : # a section of an earlier candidate of this version: replaced by the full section below
    else
      keep+=("$sec")
    fi
  else
    keep+=("$sec")
  fi
done

generated="$work/generated.md"
if [ -n "$last_stable" ]; then
  context_err="$work/cliff.err"
  cliff --unreleased --tag "v$version" --strip header >"$generated" 2>"$context_err" || {
    cat "$context_err" >&2
    die "$EX_STATE" "git-cliff failed"
  }
else
  printf '## [%s] - %s\n' "$version" "$date" >"$generated"
fi

heading="## [$version] - $date"
merged="$work/merged.md"
awk -v heading="$heading" -f "$RELEASE_SCRIPTS_DIR/changelog-merge.awk" "$generated" ${drafts[@]+"${drafts[@]}"} >"$merged"
if ! grep -q '^### ' "$merged"; then
  printf '\n_No notable changes._\n' >>"$merged"
fi

{
  cat "$work/sections/header"
  cat "$merged"
  for sec in ${keep[@]+"${keep[@]}"}; do
    printf '\n'
    cat "$sec"
  done
} >"$work/CHANGELOG.new"
# Runs of blank lines become one blank line; none at the end of the file.
awk '/^[[:space:]]*$/ { pending = 1; next } { if (printed && pending) print ""; pending = 0; print; printed = 1 }' "$work/CHANGELOG.new" >CHANGELOG.md

# --- 4. the release notes --------------------------------------------------------------------------------------------

mkdir -p "$(dirname "$notes_file")"
intro="docs/release-notes/v$version.md"
{
  if [ -f "$intro" ]; then
    # Links such as ](../WORKFLOWS.md) work in the repository but not in a release page: point them at the tag.
    sed -e "s|](\.\./\.\./|](${repo_url}/blob/v${version}/|g" -e "s|](\.\./|](${repo_url}/blob/v${version}/docs/|g" "$intro"
    printf '\n'
  fi
  if [ -n "$last_stable" ] || [ ! -f "$intro" ]; then
    printf "## What's changed\n\n"
    tail -n +2 "$merged" | sed -e '/./,$!d'
    if [ -n "$last_stable" ]; then
      printf '\n**Full changelog**: %s/compare/%s...v%s\n' "$repo_url" "$last_stable" "$version"
    fi
  fi
  if [ ! -f "$intro" ] && [ -f docs/release-notes/install.md ]; then
    printf '\n'
    sed -e "s|@VERSION@|$version|g" -e "s|@REPO_URL@|$repo_url|g" docs/release-notes/install.md
  fi
} >"$notes_file"

# --- 5. verification -------------------------------------------------------------------------------------------------

if [ "${PREPARE_SKIP_CARGO:-}" != 1 ]; then
  metadata_flags=(--locked --format-version 1)
  if [ "${PREPARE_OFFLINE:-}" = 1 ]; then metadata_flags+=(--offline); fi
  cargo metadata "${metadata_flags[@]}" >"$work/metadata.json" 2>"$work/metadata.err" || {
    cat "$work/metadata.err" >&2
    die "$EX_STATE" "cargo metadata --locked failed: Cargo.lock is not consistent with the manifests"
  }
  wrong="$(jq -r --arg v "$version" --arg names "$names" '($names | split(" ")) as $ours | [.packages[] | select(.name as $n | $ours | index($n)) | select(.version != $v) | .name] | join(" ")' "$work/metadata.json")"
  [ -z "$wrong" ] || die "$EX_STATE" "after the update these workspace packages are not at $version: $wrong"
fi

# Only the files this script owns may have changed, and Cargo.lock only in its `version` lines.
changed="$(git status --porcelain --untracked-files=no | sed 's/^...//')"
while IFS= read -r f; do
  [ -z "$f" ] && continue
  case "$f" in
    Cargo.toml | Cargo.lock | CHANGELOG.md | crates/*/Cargo.toml) ;;
    *) if [ "$allow_dirty" = 1 ]; then continue; fi; die "$EX_STATE" "unexpected change in $f" ;;
  esac
done <<<"$changed"
bad_lines="$(git diff -U0 -- Cargo.lock | grep -E '^[+-]' | grep -vE '^(\+\+\+|---) ' | grep -vE '^[+-]version = "' || true)"
[ -z "$bad_lines" ] || die "$EX_STATE" "Cargo.lock changed in more than the workspace versions:
$bad_lines"

note "prepared $version (date $date)"
git diff --stat -- Cargo.toml Cargo.lock CHANGELOG.md crates >&2 || true
note "release notes: $notes_file"

#!/usr/bin/env bash
# Prints the version of the next release (to stdout, nothing else) and explains the decision (to stderr).
#
# Usage: next-version.sh [--bump auto|major|minor|patch] [--version X.Y.Z[-rc.N]] [--prerelease alpha|beta|rc]
#                        [--reason-file FILE] [--allow-dirty]
#
# The version is computed from the Conventional Commits since the previous stable release tag (vX.Y.Z), by git-cliff
# configured in cliff.toml. While the major version is 0: a breaking change or `feat` -> minor, `fix`/`perf` -> patch;
# any other commit type alone is "nothing to release". From 1.0.0 on a breaking change bumps the major version.
# 1.0.0 itself is never computed: ask for it with `--bump major` or `--version 1.0.0`.
#
#   --bump B           override the computed bump (auto is the default)
#   --version V        use exactly this version (X.Y.Z or X.Y.Z-alpha.N / -beta.N / -rc.N); it must be higher than every tag
#   --prerelease KIND  append -KIND.N to the computed version; N is one more than the highest existing tag for that version
#   --reason-file F    also write the decision as a Markdown list to F
#   --allow-dirty      do not refuse a working tree with uncommitted changes to tracked files
#
# No previous tag: the version in crates/app/Cargo.toml (without a pre-release suffix) is the first release; old history
# is not parsed. Previous tags that are not stable (v0.2.0-rc.1) are ignored when looking for the previous release, but the
# version in Cargo.toml must equal the highest tag of any kind (a release bumps the manifests).
#
# Exit status: 0 version printed; 10 nothing to release; 64 wrong arguments; 65 the repository is not in a releasable
# state (dirty tree, shallow clone, tag/Cargo mismatch, version not higher than the tags); 69 a tool is missing.
# Environment: CLIFF_CONFIG (default <repo>/cliff.toml), GIT_CLIFF (binary).
set -euo pipefail

SELF="next-version.sh"
# shellcheck source=scripts/release/lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

bump="auto" explicit="" prerelease="" reason_file="" allow_dirty=0
while [ $# -gt 0 ]; do
  case "$1" in
    --bump)
      [ $# -ge 2 ] || die "$EX_USAGE" "--bump needs a value"
      bump="$2"
      shift 2
      ;;
    --version)
      [ $# -ge 2 ] || die "$EX_USAGE" "--version needs a value"
      explicit="${2#v}"
      shift 2
      ;;
    --prerelease)
      [ $# -ge 2 ] || die "$EX_USAGE" "--prerelease needs alpha, beta or rc"
      prerelease="$2"
      shift 2
      ;;
    --reason-file)
      [ $# -ge 2 ] || die "$EX_USAGE" "--reason-file needs a path"
      reason_file="$2"
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
    *) die "$EX_USAGE" "unknown argument: $1 (see --help)" ;;
  esac
done

case "$bump" in auto | major | minor | patch) ;; *) die "$EX_USAGE" "--bump must be auto, major, minor or patch (got '$bump')" ;; esac
case "$prerelease" in "" | alpha | beta | rc) ;; *) die "$EX_USAGE" "--prerelease must be alpha, beta or rc (got '$prerelease')" ;; esac
if [ -n "$explicit" ]; then
  [ "$bump" = auto ] || die "$EX_USAGE" "use either --bump or --version, not both"
  is_version "$explicit" || die "$EX_USAGE" "'$explicit' is not a release version (use X.Y.Z or X.Y.Z-rc.N, no leading zeros, no 'v')"
  [ -z "$prerelease" ] || die "$EX_USAGE" "--prerelease cannot be combined with --version (put the suffix in the version)"
fi

require_tool git
require_tool jq "used to read git-cliff's output"
find_git_cliff

root="$(git rev-parse --show-toplevel 2>/dev/null)" || die "$EX_STATE" "not inside a git repository"
cd "$root"
CLIFF_CONFIG="${CLIFF_CONFIG:-$root/cliff.toml}"
[ -f "$CLIFF_CONFIG" ] || die "$EX_STATE" "$CLIFF_CONFIG not found"

# --- state of the repository --------------------------------------------------------------------------------------

git rev-parse --verify -q HEAD >/dev/null || die "$EX_STATE" "the repository has no commits"
if [ "$(git rev-parse --is-shallow-repository)" = true ]; then
  die "$EX_STATE" "this is a shallow clone: tags and history are missing, the version would be wrong (fetch with full history)"
fi
if [ "$allow_dirty" = 0 ] && [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  die "$EX_STATE" "the working tree has uncommitted changes to tracked files (commit or stash them, or use --allow-dirty)"
fi

repo_versions
[ "$APP_VERSION" = "$CORE_VERSION" ] ||
  die "$EX_STATE" "$APP_MANIFEST ($APP_VERSION) and $CORE_MANIFEST ($CORE_VERSION) have different versions"
is_version "$APP_VERSION" || die "$EX_STATE" "the version '$APP_VERSION' in $APP_MANIFEST is not X.Y.Z or X.Y.Z-rc.N"

last_any="$(highest_tag)"
last_stable="$(highest_tag stable)"
if [ -n "$last_any" ] && [ "${last_any#v}" != "$APP_VERSION" ]; then
  die "$EX_STATE" "tag/Cargo mismatch: the highest release tag is $last_any but $APP_MANIFEST says $APP_VERSION"
fi

# A commit that is already released cannot be released again, except that a release candidate may be promoted to the
# final release (or followed by another candidate) without new commits.
if [ -n "$last_any" ] && is_stable "${last_any#v}" && [ "$(git rev-parse "$last_any^{commit}")" = "$(git rev-parse HEAD)" ]; then
  note "HEAD is already released as $last_any: no commits since."
  exit "$EX_NOTHING"
fi

if [ -n "$last_stable" ]; then
  since_count="$(git rev-list --count "$last_stable..HEAD")"
  previous_text="$last_stable"
  base="${last_stable#v}"
else
  since_count="$(git rev-list --count HEAD)"
  previous_text="none"
  base="$(core_of "$APP_VERSION")"
fi

# --- what the commits say -------------------------------------------------------------------------------------------

# jq program: one line "<level>\t<sha>" per commit that can release. The group names come from cliff.toml.
# (Kept out of a command substitution on purpose: bash 3.2 mis-parses quotes and parentheses inside $( ).)
read -r -d '' JQ_DECISIVE <<'JQ' || true
(.[0].commits // [])[]
| (if .breaking then "breaking"
   elif (.group | test("Added$")) then "feat"
   elif (.group | test("Fixed$|Performance$")) then "fix"
   else "other" end) as $level
| select($level != "other")
| "\($level)\t\(.id)"
JQ

# Lines "<level>\t<sha>" for every commit that can release: breaking, feat, fix (fix also covers perf).
decisive=""
level="none"
if [ -n "$last_stable" ]; then
  context_err="$(mktemp)"
  trap 'rm -f "$context_err"' EXIT
  context="$(cliff --unreleased --context 2>"$context_err")" || {
    cat "$context_err" >&2
    die "$EX_STATE" "git-cliff failed"
  }
  cliff_previous="$(jq -r '.[0].previous.version // ""' <<<"$context")"
  [ -z "$cliff_previous" ] || [ "$cliff_previous" = "$last_stable" ] ||
    die "$EX_STATE" "git-cliff sees the previous release as $cliff_previous but git says $last_stable"
  decisive="$(jq -r "$JQ_DECISIVE" <<<"$context")"
  if grep -q '^breaking' <<<"$decisive"; then
    level="breaking"
  elif grep -q '^feat' <<<"$decisive"; then
    level="feat"
  elif [ -n "$decisive" ]; then
    level="fix"
  fi
fi

# --- the version ---------------------------------------------------------------------------------------------------

rank() { case "$1" in patch) echo 1 ;; minor) echo 2 ;; major) echo 3 ;; *) echo 0 ;; esac; }
major_of_base() { split_version "$base" && printf '%s' "$V_MAJOR"; }

# The bump that the commits ask for (empty when they ask for none).
auto_bump=""
case "$level" in
  breaking) if [ "$(major_of_base)" = 0 ]; then auto_bump="minor"; else auto_bump="major"; fi ;;
  feat) auto_bump="minor" ;;
  fix) auto_bump="patch" ;;
esac

reason=""
NL='
'
add_reason() {
  if [ -n "$reason" ]; then reason="$reason$NL$1"; else reason="$1"; fi
}

computed_bump="" # major|minor|patch, empty for the first release
if [ -n "$explicit" ]; then
  version="$explicit"
  add_reason "exact version requested: $version"
elif [ "$bump" != auto ]; then
  version="$(bump_version "$base" "$bump")"
  computed_bump="$bump"
  add_reason "bump requested: $bump (from $base)"
  if [ "$bump" = major ] && [ "$(major_of_base)" = 0 ]; then
    add_reason "this is the 1.0.0 decision: it is never computed from commits"
  fi
elif [ -z "$last_stable" ]; then
  version="$base"
  add_reason "first release: no stable tag yet, so the version in $APP_MANIFEST ($base) is released; old history is not parsed"
else
  if [ "$level" = none ]; then
    note "Nothing to release since $last_stable: $since_count commit(s), none of them feat, fix, perf or breaking."
    if [ -n "$reason_file" ]; then
      printf -- '- Nothing to release since `%s`: %s commit(s), none of them `feat`, `fix`, `perf` or a breaking change.\n' "$last_stable" "$since_count" >"$reason_file"
    fi
    exit "$EX_NOTHING"
  fi
  computed_bump="$auto_bump"
  version="$(bump_version "$base" "$computed_bump")"
  # git-cliff has its own opinion (it is what `--bumped-version` prints); the two must agree.
  cliff_version="$(cliff --unreleased --bumped-version 2>/dev/null || true)"
  [ "${cliff_version#v}" = "$version" ] ||
    die "$EX_STATE" "internal check failed: git-cliff proposes '${cliff_version:-nothing}' but the rules give $version"
  add_reason "$computed_bump bump from $last_stable, because of:"
fi

if [ -n "$prerelease" ]; then
  highest=0
  while IFS= read -r t; do
    n="${t##*.}"
    case "$n" in '' | *[!0-9]*) continue ;; esac
    if [ "$n" -gt "$highest" ]; then highest="$n"; fi
  done < <(git tag --list "v$version-$prerelease.*")
  version="$version-$prerelease.$((highest + 1))"
  add_reason "pre-release: $prerelease number $((highest + 1))"
fi

# A bump that is lower than the commits imply is allowed, but the person should know.
if [ -z "$explicit" ] && [ "$bump" != auto ] && [ -n "$auto_bump" ] && [ "$(rank "$bump")" -lt "$(rank "$auto_bump")" ]; then
  warn "the commits since $previous_text ask for a $auto_bump bump; the requested '$bump' is lower"
fi

# --- checks on the result --------------------------------------------------------------------------------------------

is_version "$version" || die "$EX_STATE" "internal error: '$version' is not a valid version"
if git rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
  die "$EX_STATE" "the tag v$version already exists"
fi
if [ -n "$last_any" ] && [ "$(version_cmp "$version" "${last_any#v}")" != 1 ]; then
  die "$EX_STATE" "$version is not higher than the latest tag $last_any"
fi

# --- explanation ------------------------------------------------------------------------------------------------------

{
  note "previous stable release: $previous_text ($since_count commit(s) since)"
  note "next version: $version"
  printf '%s\n' "$reason" | sed 's/^/  /' >&2
  if [ -n "$decisive" ]; then
    while IFS="$(printf '\t')" read -r kind sha; do
      subject="$(git show -s --format=%s "$sha")"
      printf '    - %s %s  [%s]\n' "$(git rev-parse --short "$sha")" "$subject" "$kind" >&2
    done <<<"$decisive"
  fi
}

if [ -n "$reason_file" ]; then
  {
    printf -- '- Previous stable release: `%s` (%s commit(s) since).\n' "$previous_text" "$since_count"
    printf -- '- Next version: **%s**.\n' "$version"
    printf '%s\n' "$reason" | sed 's/^/- /'
    if [ -n "$decisive" ]; then
      while IFS="$(printf '\t')" read -r kind sha; do
        printf '  - `%s` %s (%s)\n' "$(git rev-parse --short "$sha")" "$(git show -s --format=%s "$sha")" "$kind"
      done <<<"$decisive"
    fi
  } >"$reason_file"
fi

printf '%s\n' "$version"

# Shared helpers of the release scripts. Source it; do not execute it. Written for bash 3.2 (macOS) and later.
# shellcheck shell=bash
# shellcheck disable=SC2034 # variables are used by the scripts that source this file

# Exit statuses shared by the scripts (documented in docs/RELEASING.md).
EX_NOTHING=10 # next-version.sh: nothing to release
EX_USAGE=64   # wrong arguments
EX_STATE=65   # the repository or the inputs are not in a state that allows a release
EX_TOOL=69    # a required tool is missing

SELF="${SELF:-$(basename "$0")}"

die() { # die STATUS MESSAGE...
  local status="$1"
  shift
  printf '%s: error: %s\n' "$SELF" "$*" >&2
  exit "$status"
}
note() { printf '%s\n' "$*" >&2; }
warn() { printf '%s: warning: %s\n' "$SELF" "$*" >&2; }

RELEASE_SCRIPTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# --- tools --------------------------------------------------------------------------------------------------------

require_tool() {
  command -v "$1" >/dev/null 2>&1 || die "$EX_TOOL" "required tool not found: $1${2:+ ($2)}"
}

# find_git_cliff: sets GIT_CLIFF_BIN. The version must be the pinned one (scripts/release/tools.env), so that the
# output is the same everywhere; GIT_CLIFF_ALLOW_ANY_VERSION=1 skips that check (local experiments only).
find_git_cliff() {
  # shellcheck source=scripts/release/tools.env
  . "$RELEASE_SCRIPTS_DIR/tools.env"
  GIT_CLIFF_BIN="${GIT_CLIFF:-git-cliff}"
  command -v "$GIT_CLIFF_BIN" >/dev/null 2>&1 ||
    die "$EX_TOOL" "git-cliff not found; run scripts/release/install-git-cliff.sh (pinned version $GIT_CLIFF_VERSION) and put it on PATH"
  if [ "${GIT_CLIFF_ALLOW_ANY_VERSION:-}" != 1 ]; then
    local have
    have="$("$GIT_CLIFF_BIN" --version 2>/dev/null || true)"
    [ "$have" = "git-cliff $GIT_CLIFF_VERSION" ] ||
      die "$EX_TOOL" "expected git-cliff $GIT_CLIFF_VERSION, found '${have:-nothing}'; run scripts/release/install-git-cliff.sh"
  fi
}

# cliff ARGS...: git-cliff with the project's configuration, quiet, no external commands.
cliff() {
  "$GIT_CLIFF_BIN" --config "$CLIFF_CONFIG" --no-exec "$@"
}

# --- semantic versions --------------------------------------------------------------------------------------------
# Accepted: X.Y.Z or X.Y.Z-(alpha|beta|rc).N with no leading zeros. Nothing else is ever released.

SEMVER_CORE='(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'
SEMVER_RE="^${SEMVER_CORE}(-(alpha|beta|rc)\\.([1-9][0-9]*))?\$"

is_version() { [[ "$1" =~ $SEMVER_RE ]]; }
is_stable() { [[ "$1" =~ ^${SEMVER_CORE}$ ]]; }

# split_version VERSION -> V_MAJOR V_MINOR V_PATCH V_PRE (e.g. "rc.2" or "") V_PRE_KIND V_PRE_NUM
split_version() {
  [[ "$1" =~ $SEMVER_RE ]] || return 1
  V_MAJOR="${BASH_REMATCH[1]}"
  V_MINOR="${BASH_REMATCH[2]}"
  V_PATCH="${BASH_REMATCH[3]}"
  V_PRE_KIND="${BASH_REMATCH[5]:-}"
  V_PRE_NUM="${BASH_REMATCH[6]:-}"
  V_PRE=""
  if [ -n "$V_PRE_KIND" ]; then V_PRE="$V_PRE_KIND.$V_PRE_NUM"; fi
  return 0
}

core_of() { # core_of VERSION -> X.Y.Z without the suffix
  printf '%s' "${1%%-*}"
}

# version_cmp A B -> prints -1, 0 or 1 (semver precedence; a pre-release sorts before its release).
version_cmp() {
  local a_major a_minor a_patch a_kind a_num b_major b_minor b_patch b_kind b_num
  split_version "$1" || return 1
  a_major=$V_MAJOR a_minor=$V_MINOR a_patch=$V_PATCH a_kind=$V_PRE_KIND a_num=$V_PRE_NUM
  split_version "$2" || return 1
  b_major=$V_MAJOR b_minor=$V_MINOR b_patch=$V_PATCH b_kind=$V_PRE_KIND b_num=$V_PRE_NUM
  local x y
  for pair in "$a_major $b_major" "$a_minor $b_minor" "$a_patch $b_patch"; do
    # shellcheck disable=SC2086 # intentional split into two numbers
    set -- $pair
    x=$1 y=$2
    if [ "$x" -lt "$y" ]; then
      echo -1
      return 0
    elif [ "$x" -gt "$y" ]; then
      echo 1
      return 0
    fi
  done
  if [ -z "$a_kind" ] && [ -z "$b_kind" ]; then
    echo 0
  elif [ -z "$a_kind" ]; then
    echo 1
  elif [ -z "$b_kind" ]; then
    echo -1
  elif [ "$a_kind" != "$b_kind" ]; then
    # alpha < beta < rc is also alphabetical order
    if [[ "$a_kind" < "$b_kind" ]]; then echo -1; else echo 1; fi
  elif [ "$a_num" -lt "$b_num" ]; then
    echo -1
  elif [ "$a_num" -gt "$b_num" ]; then
    echo 1
  else
    echo 0
  fi
}

# bump_version VERSION major|minor|patch -> the next stable version (a pre-release suffix is dropped first).
bump_version() {
  split_version "$1" || return 1
  case "$2" in
    major) printf '%s.0.0' "$((V_MAJOR + 1))" ;;
    minor) printf '%s.%s.0' "$V_MAJOR" "$((V_MINOR + 1))" ;;
    patch) printf '%s.%s.%s' "$V_MAJOR" "$V_MINOR" "$((V_PATCH + 1))" ;;
    *) return 1 ;;
  esac
}

# --- the repository -----------------------------------------------------------------------------------------------

# manifest_version FILE -> the `version` in the [package] table.
manifest_version() {
  awk '
    /^\[/ { in_package = ($0 == "[package]") }
    in_package && /^version[[:space:]]*=[[:space:]]*"/ {
      v = $0
      sub(/^version[[:space:]]*=[[:space:]]*"/, "", v)
      sub(/".*$/, "", v)
      print v
      exit
    }
  ' "$1"
}

APP_MANIFEST="crates/app/Cargo.toml"
CORE_MANIFEST="crates/core/Cargo.toml"

# repo_versions: sets APP_VERSION and CORE_VERSION from the manifests in the current repository.
repo_versions() {
  [ -f "$APP_MANIFEST" ] || die "$EX_STATE" "$APP_MANIFEST not found (run inside the repository)"
  [ -f "$CORE_MANIFEST" ] || die "$EX_STATE" "$CORE_MANIFEST not found (run inside the repository)"
  APP_VERSION="$(manifest_version "$APP_MANIFEST")"
  CORE_VERSION="$(manifest_version "$CORE_MANIFEST")"
  [ -n "$APP_VERSION" ] || die "$EX_STATE" "cannot read the version from $APP_MANIFEST"
  [ -n "$CORE_VERSION" ] || die "$EX_STATE" "cannot read the version from $CORE_MANIFEST"
}

# release_tags: prints every tag that is a release tag (vX.Y.Z or vX.Y.Z-rc.N) and is reachable from HEAD, unsorted.
release_tags() {
  local t
  git tag --merged HEAD --list 'v*' | while IFS= read -r t; do
    if is_version "${t#v}"; then printf '%s\n' "$t"; fi
  done
}

# highest_tag [stable]: prints the highest release tag reachable from HEAD (only stable ones with the argument).
highest_tag() {
  local best="" t
  while IFS= read -r t; do
    if [ "${1:-}" = stable ] && ! is_stable "${t#v}"; then continue; fi
    if [ -z "$best" ] || [ "$(version_cmp "${t#v}" "${best#v}")" = 1 ]; then best="$t"; fi
  done < <(release_tags)
  printf '%s' "$best"
}

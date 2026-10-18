#!/usr/bin/env bash
# The "plan" step of the release workflow, kept in a script so that it can be linted, tested and run locally.
#
# Input (environment, never interpolated into a command line by the workflow):
#   INPUT_BUMP        auto | patch | minor | major   (default auto)
#   INPUT_VERSION     optional exact version
#   INPUT_PRERELEASE  true | false                  (default false)
# Output: lines "release=true|false", "version=", "tag=", "prerelease=true|false", "sha=" appended to $GITHUB_OUTPUT (or
# printed when it is not set), and a Markdown report in $GITHUB_STEP_SUMMARY (or printed).
# Exit status: 0 also for "nothing to release" (release=false); anything else is an error.
set -euo pipefail

SELF="ci-plan.sh"
# shellcheck source=scripts/release/lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

bump="${INPUT_BUMP:-auto}" version_in="${INPUT_VERSION:-}" prerelease_in="${INPUT_PRERELEASE:-false}"
out="${GITHUB_OUTPUT:-/dev/stdout}"
summary="${GITHUB_STEP_SUMMARY:-/dev/stdout}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

args=()
case "$bump" in auto) ;; patch | minor | major) args+=(--bump "$bump") ;; *) die "$EX_USAGE" "bump must be auto, patch, minor or major" ;; esac
if [ -n "$version_in" ]; then args+=(--version "$version_in"); fi
case "$prerelease_in" in
  true) if [ -z "$version_in" ]; then args+=(--prerelease rc); fi ;;
  false | "") ;;
  *) die "$EX_USAGE" "prerelease must be true or false" ;;
esac

status=0
version="$("$RELEASE_SCRIPTS_DIR/next-version.sh" ${args[@]+"${args[@]}"} --reason-file "$work/reason.md" 2>"$work/err.txt")" || status=$?

{
  echo "## Release plan"
  echo
  echo "Inputs: bump \`$bump\`, version \`${version_in:-(computed)}\`, pre-release \`$prerelease_in\`."
  echo
} >>"$summary"

case "$status" in
  0)
    {
      echo "**Next version: \`$version\`** (tag \`v$version\`)."
      echo
      cat "$work/reason.md"
      echo
    } >>"$summary"
    {
      echo "release=true"
      echo "version=$version"
      echo "tag=v$version"
      case "$version" in *-*) echo "prerelease=true" ;; *) echo "prerelease=false" ;; esac
      echo "sha=$(git rev-parse HEAD)"
    } >>"$out"
    ;;
  "$EX_NOTHING")
    {
      echo "**Nothing to release.**"
      echo
      cat "$work/reason.md" 2>/dev/null || cat "$work/err.txt"
      echo
      echo "Only \`feat\`, \`fix\`, \`perf\` and breaking changes make a release. Use \`bump\` or \`version\` to force one."
    } >>"$summary"
    echo "release=false" >>"$out"
    ;;
  *)
    {
      echo "**The plan failed** (status $status):"
      echo
      echo '```'
      cat "$work/err.txt"
      echo '```'
    } >>"$summary"
    cat "$work/err.txt" >&2
    exit "$status"
    ;;
esac
cat "$work/err.txt" >&2

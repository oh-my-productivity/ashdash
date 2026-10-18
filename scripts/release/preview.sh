#!/usr/bin/env bash
# What would the next release be, and what would its notes say? Prints Markdown to stdout. Changes nothing in the
# repository it is run in (all work happens in a temporary clone).
#
# Usage: preview.sh [--message-file FILE]
#
#   --message-file F   the message of the squash commit that a pull request would add to main (title, blank line, body);
#                      the report then compares the release before and after it.
#
# Used by .github/workflows/release-preview.yml. It never runs code from a pull request: the workflow checks out the
# base branch, and the pull request only contributes the text in FILE, which is parsed, never executed.
# Environment: as next-version.sh / prepare.sh; GITHUB_REPOSITORY makes the links point at the right repository.
set -euo pipefail

SELF="preview.sh"
# shellcheck source=scripts/release/lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

message_file=""
while [ $# -gt 0 ]; do
  case "$1" in
    --message-file)
      [ $# -ge 2 ] || die "$EX_USAGE" "--message-file needs a path"
      message_file="$2"
      shift 2
      ;;
    -h | --help)
      sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) die "$EX_USAGE" "unknown argument: $1" ;;
  esac
done
[ -z "$message_file" ] || [ -f "$message_file" ] || die "$EX_USAGE" "$message_file does not exist"

src="$(git rev-parse --show-toplevel 2>/dev/null)" || die "$EX_STATE" "not inside a git repository"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git clone --quiet --no-hardlinks "$src" "$work/repo"
cd "$work/repo"

# next_version OUTVAR: runs next-version.sh; sets STATUS, VERSION_OUT and REASON (Markdown list).
next_version() {
  STATUS=0
  VERSION_OUT="$("$RELEASE_SCRIPTS_DIR/next-version.sh" --reason-file "$work/reason.md" 2>"$work/err.txt")" || STATUS=$?
  REASON="$(cat "$work/reason.md" 2>/dev/null || true)"
  rm -f "$work/reason.md"
}

next_version
before_status=$STATUS before_version=$VERSION_OUT before_reason=$REASON
[ "$before_status" = 0 ] || [ "$before_status" = "$EX_NOTHING" ] || { cat "$work/err.txt" >&2; die "$EX_STATE" "next-version.sh failed"; }

echo "## Release preview"
echo

after_status=$before_status after_version=$before_version after_reason=$before_reason
if [ -n "$message_file" ]; then
  git -c user.name="preview" -c user.email="preview@invalid.invalid" commit --quiet --allow-empty -F "$message_file"
  next_version
  after_status=$STATUS after_version=$VERSION_OUT after_reason=$REASON
  [ "$after_status" = 0 ] || [ "$after_status" = "$EX_NOTHING" ] || { cat "$work/err.txt" >&2; die "$EX_STATE" "next-version.sh failed"; }

  subject="$(head -n 1 "$message_file")"
  echo "Squash-merging this pull request adds one commit to \`main\` with this header:"
  echo
  echo '```'
  printf '%s\n' "$subject"
  echo '```'
  echo
  if [ "$before_status" = 0 ]; then
    echo "- Without this pull request the next release would be **$before_version**."
  else
    echo "- Without this pull request there is nothing to release."
  fi
  if [ "$after_status" = 0 ]; then
    if [ "$before_status" = 0 ] && [ "$before_version" = "$after_version" ]; then
      echo "- With it the next release is still **$after_version**: it does not change the version."
    else
      echo "- With it the next release would be **$after_version**."
    fi
  else
    echo "- With it there is still nothing to release (only \`feat\`, \`fix\`, \`perf\` and breaking changes make a release)."
  fi
  echo
fi

if [ "$after_status" != 0 ]; then
  echo "**Nothing to release** at the moment."
  echo
  echo "$after_reason"
  exit 0
fi

echo "### Why $after_version"
echo
echo "$after_reason"
echo

if PREPARE_SKIP_CARGO=1 "$RELEASE_SCRIPTS_DIR/prepare.sh" "$after_version" --notes-file "$work/notes.md" >"$work/prepare.out" 2>"$work/prepare.err"; then
  echo "### Files a release would change"
  echo
  echo '```'
  git diff --stat | sed 's/^ //'
  echo '```'
  echo
  echo "### Release notes (preview)"
  echo
  echo '````markdown'
  cat "$work/notes.md"
  echo '````'
else
  echo "### The release could not be prepared"
  echo
  echo '```'
  cat "$work/prepare.err"
  echo '```'
fi

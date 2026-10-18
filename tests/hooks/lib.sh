# shellcheck shell=bash
# Tiny assertion helpers shared by the suites in this folder. Sourced, not executed.
# Counters live in the sourcing script: PASSED, FAILED. Print a summary with `summary`.
PASSED=0
FAILED=0
# shellcheck disable=SC2034  # used by the suites that source this file
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)

pass() { PASSED=$((PASSED + 1)); }
fail() { FAILED=$((FAILED + 1)); printf 'FAIL: %s\n' "$1" >&2; [ -z "${2-}" ] || printf '%s\n' "$2" | sed 's/^/      /' >&2; }

# expect_rc <expected> <description> <actual> [output]
expect_rc() {
  if [ "$3" = "$1" ]; then pass; else fail "$2 (expected exit $1, got $3)" "${4-}"; fi
}

# expect_contains <description> <haystack> <needle>
expect_contains() {
  case $2 in *"$3"*) pass ;; *) fail "$1 (output lacks: $3)" "$2" ;; esac
}

expect_lacks() {
  case $2 in *"$3"*) fail "$1 (output has: $3)" "$2" ;; *) pass ;; esac
}

summary() { # summary <suite name>
  printf '%s: %d passed, %d failed\n' "$1" "$PASSED" "$FAILED"
  [ "$FAILED" = 0 ]
}

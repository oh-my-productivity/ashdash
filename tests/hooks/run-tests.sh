#!/usr/bin/env bash
# All tests of the commit convention and the git hooks (scripts/commit-lint.sh, .githooks/, scripts/setup-hooks.sh).
# Run by scripts/check.sh and by the `commit-lint` workflow. Needs bash, git, awk and rustfmt; no network, no cargo build.
set -u
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
rc=0
for suite in commit-lint hooks; do
  echo "== $suite"
  "$here/$suite.test.sh" || rc=1
done
[ "$rc" = 0 ] && echo "hooks tests: OK"
exit "$rc"

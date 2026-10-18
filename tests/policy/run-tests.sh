#!/usr/bin/env bash
# The tests of the testing tooling: the policy check (policy-check.test.sh) and scripts/ci-run.sh (ci-run.test.sh). Pure text and dry runs:
# no cargo, no network, no GUI, about 10 s. Run by scripts/check.sh and by the `policy` job of ci.yml.
set -u
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
rc=0
for suite in policy-check ci-run; do
  echo "== $suite"
  "$here/$suite.test.sh" || rc=1
done
[ "$rc" = 0 ] && echo "policy tests: OK"
exit "$rc"

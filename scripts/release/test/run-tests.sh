#!/usr/bin/env bash
# Runs every test of the release scripts. Needs the pinned git-cliff on PATH (scripts/release/install-git-cliff.sh);
# without it the tests are skipped locally and fail when REQUIRE_GIT_CLIFF=1 (CI sets it).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
status=0
for t in "$here"/test-*.sh; do
  echo "##### $(basename "$t")"
  "$t" || status=1
done
exit "$status"

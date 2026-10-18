#!/usr/bin/env bash
# The smoke test (tests/e2e/smoke.sh) on a GitHub-hosted macOS runner:
#   bash tests/e2e/ci/run-smoke.sh [smoke.sh options]
# Builds the debug binary and the native bundle, launches the bundle in the runner's GUI session and checks `ashdash show/search`, the
# ashdash:// scheme, the global hotkey through System Events, window state, the login item and `quit`. scripts/check.sh is NOT run again
# here (the `check` job does that): --quick is passed. Exit status is smoke.sh's: 1 when a stage FAILs, SKIPs do not fail.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=common.sh
. "$here/common.sh"
ci_require_hosted
cd "$CI_ROOT" || exit 1

art=$(ci_artifacts smoke)
export E2E_ARTIFACTS=$art
export E2E_AUTO_TCC=${E2E_AUTO_TCC:-1}   # tcc-grant.sh accepts a hosted runner (and refuses where it is not a VirtualMac, e.g. macos-15-intel)
bash tests/e2e/smoke.sh --quick "$@"
rc=$?

{
  echo "### GUI smoke on $(sw_vers -productName) $(sw_vers -productVersion) ($(uname -m))"
  echo '```'
  cat "$art/smoke-report.txt" 2>/dev/null
  echo '```'
} | ci_summary
[ "$rc" = 0 ] && ci_drop_screenshots "$art"
exit "$rc"

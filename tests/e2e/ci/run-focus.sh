#!/usr/bin/env bash
# The focus checks (does the launcher hide when another app is activated, Quick Look, panel, click, idle) on a GitHub-hosted macOS runner:
#   bash tests/e2e/ci/run-focus.sh [regress|probe] [options of that script]       (default: regress)
# regress.sh is the regression check (Quick Look, panel, click, idle); probe.sh repeats the 3.4.x trials with a trace of the app's own log
# (docs/spikes/ui-focus.md). Both build Ashdash.app and drive the real keyboard through Terminal, so they run on CI only.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=common.sh
. "$here/common.sh"
ci_require_hosted
cd "$CI_ROOT" || exit 1

mode=regress
case ${1:-} in regress | probe) mode=$1; shift ;; esac
art=$(ci_artifacts focus)
export QA_ART=$art E2E_ARTIFACTS=$art E2E_AUTO_TCC=${E2E_AUTO_TCC:-1}
bash "tests/e2e/focus/$mode.sh" "$@" 2>&1 | tee "$art/$mode.log"
rc=${PIPESTATUS[0]}
{
  echo "### Focus $mode on $(sw_vers -productName) $(sw_vers -productVersion) ($(uname -m))"
  echo '```'
  tail -n 40 "$art/$mode.log"
  echo '```'
} | ci_summary
[ "$rc" = 0 ] && ci_drop_screenshots "$art"
exit "$rc"

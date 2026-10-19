#!/usr/bin/env bash
# The launcher window screenshots (tests/e2e/screenshots/launcher-look.sh) on a GitHub-hosted macOS runner:
#   bash tests/e2e/ci/run-launcher-look.sh
# The empty launcher, the launcher with results ("saf"), with only web-search fallbacks (`fbonly`) and with long path
# subtitles (`longpath`) go to $RUNNER_TEMP/ashdash-ci/launcher-look/shots/*.png, which the `launcher-look` job
# of e2e.yml uploads as an artifact (7 days), together with window.txt (the window's size in points). They are evidence for a person to
# look at; the job fails when the script cannot take them or when the edge check finds a light pixel on the window's edge.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=common.sh
. "$here/common.sh"
ci_require_hosted
cd "$CI_ROOT" || exit 1

art=$(ci_artifacts launcher-look)
export E2E_ARTIFACTS=$art E2E_AUTO_TCC=${E2E_AUTO_TCC:-1}
bash tests/e2e/screenshots/launcher-look.sh
rc=$?
n=$(find "$art/shots" -name '*.png' 2>/dev/null | wc -l | tr -d ' ')
{
  echo "### Launcher window on $(sw_vers -productName) $(sw_vers -productVersion) ($(uname -m))"
  echo "$n screenshots in the artifact \`launcher-look\` (empty state, with results, only fallbacks, long path subtitles)."
  echo '```'
  cat "$art/window.txt" 2>/dev/null
  echo '```'
  echo "Pixel check of the window edges (tests/e2e/screenshots/edge-check.py):"
  echo '```'
  cut -c1-220 "$art/edge-check.txt" 2>/dev/null
  echo '```'
} | ci_summary
[ "$n" -ge 20 ] || { echo "run-launcher-look: only $n screenshots were taken (expected at least 20)" >&2; rc=1; }
exit "$rc"

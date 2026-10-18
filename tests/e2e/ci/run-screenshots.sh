#!/usr/bin/env bash
# The screenshots of the preferences window (tests/e2e/screenshots/prefs-shots.sh) on a GitHub-hosted macOS runner:
#   bash tests/e2e/ci/run-screenshots.sh
# Every page in dark and light (and at two window sizes) goes to $RUNNER_TEMP/ashdash-ci/screenshots/shots/*.png, which the `screenshots`
# job of e2e.yml uploads as an artifact (7 days). They are evidence for a person to look at; nothing compares them with reference images,
# so the job fails only when the script cannot take them (no window, no screenshot).
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=common.sh
. "$here/common.sh"
ci_require_hosted
cd "$CI_ROOT" || exit 1

art=$(ci_artifacts screenshots)
export E2E_ARTIFACTS=$art E2E_AUTO_TCC=${E2E_AUTO_TCC:-1}
bash tests/e2e/screenshots/prefs-shots.sh
rc=$?
n=$(find "$art/shots" -name '*.png' 2>/dev/null | wc -l | tr -d ' ')
expected=${SHOT_EXPECTED_MIN:-40}
{
  echo "### Preferences window screenshots on $(sw_vers -productName) $(sw_vers -productVersion) ($(uname -m))"
  echo "$n screenshots in the artifact \`screenshots\` (dark and light, every page)."
} | ci_summary
if [ "$n" -lt "$expected" ]; then
  echo "run-screenshots: only $n screenshots were taken (expected at least $expected)" >&2
  rc=1
fi
exit "$rc"

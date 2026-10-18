#!/usr/bin/env bash
# The soak (tests/e2e/qa/soak.sh) on a GitHub-hosted macOS runner: show, type, hide every INTERVAL seconds for SECONDS.
#   bash tests/e2e/ci/run-soak.sh [seconds=7200] [interval=30]
# Exit status 1 when a hotkey press missed, the app crashed or died, or the hotkey stopped working at the end.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=common.sh
. "$here/common.sh"
ci_require_hosted
cd "$CI_ROOT" || exit 1

secs=${1:-7200} interval=${2:-30}
art=$(ci_artifacts soak)
export QA_ART=$art QA_RESULTS=$art/results.tsv E2E_ARTIFACTS=$art SOAK_DIR=$art
: >"$QA_RESULTS"
if ! bash tests/e2e/qa/prep.sh 2>&1 | tee "$art/prep.log"; then ci_die "prep.sh failed"; fi
[ -d /Applications/Ashdash.app ] || ci_die "prep.sh did not install /Applications/Ashdash.app (see prep.log)"
bash tests/e2e/qa/soak.sh "$secs" "$interval"

python3 "$here/finalize.py" "$QA_RESULTS" "$here/blocked.tsv" "$art/results-final.tsv" "$art/summary.md" --require SOAK >"$art/finalize.out" 2>&1
rc=$?
cat "$art/finalize.out"
{
  echo "### Soak on $(sw_vers -productName) $(sw_vers -productVersion) ($(uname -m)): ${secs} s, every ${interval} s"
  cat "$art/result.txt" 2>/dev/null
  echo; echo '```'; cat "$art/samples.tsv" 2>/dev/null; echo '```'
  cat "$art/summary.md"
} | ci_summary
[ "$rc" = 0 ] && ci_drop_screenshots "$art"
exit "$rc"

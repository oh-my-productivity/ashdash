#!/usr/bin/env bash
# The scripted QA checklist (tests/lab/qa/checklist.sh) on a GitHub-hosted macOS runner, without tart or ssh:
#   bash tests/lab/ci/run-checklist.sh [section ...]       (sections as in checklist.sh; default: all)
# Steps: qa/prep.sh (TCC grants for the lab tools, native bundle, install to /Applications), checklist.sh, then finalize.py: rows listed in
# blocked.tsv that FAIL become BLOCKED with the reason, any other FAIL fails the job. Screenshots are kept only when the job fails.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=common.sh
. "$here/common.sh"
ci_require_hosted
cd "$CI_ROOT" || exit 1

art=$(ci_artifacts checklist)
export QA_ART=$art QA_RESULTS=$art/results.tsv LAB_ARTIFACTS=$art
export CI_IMAGE="macos$(sw_vers -productVersion | cut -d. -f1)"   # blocked.tsv can limit a rule to one image
: >"$QA_RESULTS"

if ! bash tests/lab/qa/prep.sh 2>&1 | tee "$art/prep.log"; then ci_die "prep.sh failed"; fi
[ -d /Applications/Ashdash.app ] || ci_die "prep.sh did not install /Applications/Ashdash.app (see prep.log)"
bash tests/lab/qa/checklist.sh "$@" 2>&1 | tee "$art/checklist.log"

python3 "$here/finalize.py" "$QA_RESULTS" "$here/blocked.tsv" "$art/results-final.tsv" "$art/summary.md" --require SETUP >"$art/finalize.out" 2>&1
rc=$?
cat "$art/finalize.out"
{ echo "### QA checklist on $(sw_vers -productName) $(sw_vers -productVersion) ($(uname -m))"; cat "$art/summary.md"; } | ci_summary
[ "$rc" = 0 ] && ci_drop_screenshots "$art"
exit "$rc"

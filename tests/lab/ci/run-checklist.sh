#!/usr/bin/env bash
# The scripted QA checklist (tests/lab/qa/checklist.sh) on a GitHub-hosted macOS runner, without tart or ssh:
#   bash tests/lab/ci/run-checklist.sh [section ...]       (sections as in checklist.sh; default: all)
# Steps: qa/prep.sh (TCC grants for the lab tools, native bundle, install to /Applications), checklist.sh, then finalize.py: rows listed in
# blocked.tsv that FAIL become BLOCKED with the reason, any other FAIL fails the job. GUI checks on a shared 3-core virtual machine are
# timing sensitive, so the sections that contain a FAILed row are run once more (CI_RETRY=0 turns that off); a row that fails twice fails
# the job, one that passes the second time is listed in the summary as flaky. Screenshots are kept only when the job fails.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=common.sh
. "$here/common.sh"
ci_require_hosted
cd "$CI_ROOT" || exit 1

art=$(ci_artifacts checklist)
export QA_ART=$art QA_RESULTS=$art/results.tsv LAB_ARTIFACTS=$art
CI_IMAGE="macos$(sw_vers -productVersion | cut -d. -f1)"   # blocked.tsv can limit a rule to one image
export CI_IMAGE
: >"$QA_RESULTS"

finalize() { python3 "$here/finalize.py" "$QA_RESULTS" "$here/blocked.tsv" "$art/results-final.tsv" "$art/summary.md" --require SETUP; }

if ! bash tests/lab/qa/prep.sh 2>&1 | tee "$art/prep.log"; then ci_die "prep.sh failed"; fi
[ -d /Applications/Ashdash.app ] || ci_die "prep.sh did not install /Applications/Ashdash.app (see prep.log)"
bash tests/lab/qa/checklist.sh "$@" 2>&1 | tee "$art/checklist.log"

retried=""
if [ "${CI_RETRY:-1}" != 0 ] && ! finalize >/dev/null 2>&1; then
  retry=$(python3 "$here/retry-sections.py" tests/lab/qa/checklist.sh "$art/results-final.tsv")
  if [ -n "$retry" ]; then
    cp "$QA_RESULTS" "$art/results-first-pass.tsv"
    cp "$art/results-final.tsv" "$art/results-final-first-pass.tsv"
    retried=$(echo "$retry" | tr '\n' ' ')
    echo "== second pass for: $retried"
    # shellcheck disable=SC2086  # a list of section names
    bash tests/lab/qa/checklist.sh $retry 2>&1 | tee -a "$art/checklist.log"
  fi
fi

finalize >"$art/finalize.out" 2>&1
rc=$?
if [ -n "$retried" ]; then
  python3 - "$art/results-final-first-pass.tsv" "$art/results-final.tsv" "$retried" >>"$art/summary.md" <<'PY'
import sys
first = {l.split("\t")[0]: l.split("\t")[1] for l in open(sys.argv[1], encoding="utf-8") if "\t" in l}
final = {l.split("\t")[0]: l.split("\t")[1] for l in open(sys.argv[2], encoding="utf-8") if "\t" in l}
print(f"\nSecond pass (sections {sys.argv[3].strip()}), rows that FAILED the first time:\n")
for rid, st in first.items():
    if st == "FAIL":
        print(f"* {rid}: FAIL -> {final.get(rid, 'missing')}" + (" (flaky)" if final.get(rid) == "PASS" else ""))
PY
fi
cat "$art/finalize.out"; [ -n "$retried" ] && tail -n +"$(grep -n '^Second pass' "$art/summary.md" | cut -d: -f1)" "$art/summary.md"
{ echo "### QA checklist on $(sw_vers -productName) $(sw_vers -productVersion) ($(uname -m))"; cat "$art/summary.md"; } | ci_summary
[ "$rc" = 0 ] && ci_drop_screenshots "$art"
exit "$rc"

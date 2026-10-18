#!/usr/bin/env bash
# The GUI performance run (scripts/qa/perf-gui.sh) on a GitHub-hosted macOS runner: cold and warm start-up, hotkey and `ashdash show`
# latency, idle CPU and memory of the real release binary, memory after use. The numbers are a table in the job summary and the
# results.jsonl in the artifact (docs/PERF.md says how to read them).
#   bash tests/e2e/ci/run-perf-gui.sh
# Env (passed to perf-gui.py): PERF_VARIANTS=empty,real,synth5000  PERF_REPS=3  PERF_IDLE_S=60  PERF_CYCLES=20  PERF_LAT_N=10
# The run drives the real keyboard through Terminal (gui-exec), like the smoke test, after tcc-grant.sh.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=common.sh
. "$here/common.sh"
ci_require_hosted
cd "$CI_ROOT" || exit 1

art=$(ci_artifacts perf-gui)
export E2E_ARTIFACTS=$art
bash tests/e2e/guest/tcc-grant.sh >"$art/tcc-grant.txt" 2>&1 || { cat "$art/tcc-grant.txt"; ci_die "tcc-grant.sh failed"; }
cargo build --release -p ashdash >"$art/build.log" 2>&1 || { tail -30 "$art/build.log"; ci_die "release build failed"; }

export PERF_OUT=$art/results
export PERF_VARIANTS=${PERF_VARIANTS:-empty,real,synth5000} PERF_REPS=${PERF_REPS:-3} PERF_IDLE_S=${PERF_IDLE_S:-60}
export PERF_CYCLES=${PERF_CYCLES:-20} PERF_LAT_N=${PERF_LAT_N:-10}
mkdir -p "$PERF_OUT"
# inside the GUI session: the synthetic key events need the Accessibility grant of Terminal
tests/e2e/guest/gui-exec --timeout 3300 -- scripts/qa/perf-gui.sh target/release/ashdash | tee "$art/perf-gui.log"
rc=${PIPESTATUS[0]}

{
  echo "### GUI performance on $(sw_vers -productName) $(sw_vers -productVersion) ($(uname -m)), $(sysctl -n hw.ncpu) CPUs"
  if [ -s "$PERF_OUT/results.jsonl" ]; then python3 "$here/perf-summary.py" "$PERF_OUT/results.jsonl"; else echo "no results (see the log in the artifact)"; fi
} | ci_summary
[ -s "$PERF_OUT/results.jsonl" ] || { echo "run-perf-gui: no measurements were taken" >&2; rc=1; }
exit "$rc"

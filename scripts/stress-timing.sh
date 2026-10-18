#!/usr/bin/env bash
# Runs the tests that used to depend on wall-clock time many times in a row while every core is kept busy, the way a slow, shared CI
# runner behaves (docs/CI.md, "Timing-sensitive tests"). It is meant for the `stress` job of ci.yml: it deliberately uses all CPUs, so do not
# run it on a developer machine.
#   scripts/stress-timing.sh [--runs N] [--slow-runs N] [--loops N] [SRC_DIR]
# SRC_DIR is the checkout to test (default: this repository); its tests are built and run, the list of tests below is this script's.
# Exit status 0 when no run failed. A Markdown table goes to $GITHUB_STEP_SUMMARY when that is set.
set -euo pipefail

runs=100
slow_runs=40
loops=""
while [ $# -gt 0 ]; do
  case "$1" in
    --runs) runs="$2"; shift 2 ;;
    --slow-runs) slow_runs="$2"; shift 2 ;;
    --loops) loops="$2"; shift 2 ;;
    -*) echo "usage: $0 [--runs N] [--slow-runs N] [--loops N] [SRC_DIR]" >&2; exit 2 ;;
    *) break ;;
  esac
done
src="$(cd "${1:-$(dirname "$0")/..}" && pwd)"
ncpu="$(sysctl -n hw.ncpu 2>/dev/null || getconf _NPROCESSORS_ONLN)"
[ -n "$loops" ] || loops=$((ncpu * 4))

# crate|binary|test|runs (binary "lib" = the unit tests of the crate; "slow" tests take about 10 s each)
tests=(
  "core|lib|providers::bookmarks::tests::scoring_thousands_of_bookmarks_per_keystroke_stays_cheap|runs"
  "core|lib|index::tests::query_latency_p95_at_5000_apps_stays_under_the_ceiling|runs"
  "core|lib|bookmarks::cache::tests::backoff_grows_exponentially_up_to_the_cap|runs"
  "core|lib|theme::tests::shared_references_cannot_make_the_parse_do_exponential_work|runs"
  "core|lib|settings_view::tests::a_burst_of_events_becomes_one_report|runs"
  "core|lib|workflow::script::tests::exited_script_with_a_lingering_pipe_holder_returns_promptly|runs"
  "core|prefs_core|watcher_reports_config_and_workflow_edits_but_not_noise|runs"
  "core|prefs_core|watcher_follows_extra_workflow_folders|runs"
  "core|scripts_security|backgrounded_grandchild_does_not_stall_the_script_filter|runs"
  "core|scripts_security|documents_default_mask_leaves_shell_metacharacters_live|runs"
  "core|scripts_security|unread_stdin_cannot_defeat_the_timeout|slow"
  "app|lib|scripting::tests::slow_triggers_do_not_block_the_caller|runs"
  "app|lib|gui::prefs::reload::tests::edits_on_disk_reach_the_engine_through_the_watcher|runs"
  "app|lib|ipc::tests::end_to_end_over_a_real_socket|runs"
)

cd "$src"
echo "== building the test binaries in $src"
bins="$(mktemp -d)"
cargo test --workspace --no-run --message-format=json 2>/dev/null | python3 -c '
import json, os, shutil, sys
out = sys.argv[1]
for line in sys.stdin:
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("reason") == "compiler-artifact" and m.get("profile", {}).get("test") and m.get("executable"):
        crate = os.path.basename(os.path.dirname(m["manifest_path"]))
        kind = "lib" if "bin" in m["target"]["kind"] or "lib" in m["target"]["kind"] else m["target"]["name"]
        shutil.copy(m["executable"], os.path.join(out, crate + "__" + kind))
' "$bins"

pids=()
cleanup() {
  if [ "${#pids[@]}" -gt 0 ]; then kill "${pids[@]}" 2>/dev/null || true; fi
  rm -rf "$bins"
}
trap cleanup EXIT

echo "== $loops busy loops on $ncpu CPUs"
for _ in $(seq "$loops"); do
  yes >/dev/null &
  pids+=("$!")
done

total_failed=0
summary="| test | runs | failed |\n|---|---|---|\n"
for entry in "${tests[@]}"; do
  IFS='|' read -r crate bin name kind <<<"$entry"
  n="$runs"
  [ "$kind" = slow ] && n="$slow_runs"
  exe="$bins/${crate}__${bin}"
  [ -x "$exe" ] || { echo "no test binary for $crate/$bin" >&2; exit 2; }
  failed=0
  for i in $(seq "$n"); do
    if ! out="$(cd "$src/crates/$crate" && CARGO_MANIFEST_DIR="$src/crates/$crate" "$exe" --exact "$name" --test-threads=1 2>&1)"; then
      failed=$((failed + 1))
      echo "--- $name, run $i failed:"
      echo "$out" | grep -E 'panicked|assert|p95|took|left|right|ratio|not seen|cost' | head -6 || true
    fi
  done
  echo "$name: $failed/$n failed"
  summary+="| \`$name\` | $n | $failed |\n"
  total_failed=$((total_failed + failed))
done

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### Timing tests under load ($loops busy loops on $ncpu CPUs)"
    printf '%b' "$summary"
  } >>"$GITHUB_STEP_SUMMARY"
fi
echo "stress-timing: $total_failed failed runs"
[ "$total_failed" -eq 0 ]

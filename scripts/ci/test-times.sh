#!/usr/bin/env bash
# Measures every Rust test alone, one after the other, and prints the slowest. It is the tool behind the classification in
# docs/TESTING.md (a test that needs more than about 2 s alone, or any measured wall-clock work, is CI-only). It runs on CI
# (the `test-times` job of stress.yml), not on a developer machine: it runs the whole suite single-threaded.
#   scripts/ci/test-times.sh [--release] [--ignored] [--top N] [--out DIR] [SRC_DIR]
#   --release   build and run in release mode (the `perf_` tests need it)
#   --ignored   run only the `#[ignore]`d tests instead of the others
#   --top N     how many tests to print (default 40)
#   --out DIR   keep the full table (times.tsv: seconds, crate, binary, test) there
# `--report-time` and `--format json` are unstable options of the test harness; setting RUSTC_BOOTSTRAP=1 for the test binaries (not
# for the build) enables them on the stable toolchain. A Markdown summary goes to $GITHUB_STEP_SUMMARY when that is set.
set -euo pipefail

release=""; mode=""; top=40; out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --release) release="--release"; shift ;;
    --ignored) mode="--ignored"; shift ;;
    --top) top="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    -*) echo "usage: $0 [--release] [--ignored] [--top N] [--out DIR] [SRC_DIR]" >&2; exit 2 ;;
    *) break ;;
  esac
done
here="$(cd "$(dirname "$0")" && pwd)"
src="$(cd "${1:-$here/../..}" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$src"

echo "== building the test binaries ${release:+(release)}"
# shellcheck disable=SC2086  # $release is empty or one flag
cargo test --workspace $release --no-run --message-format=json 2>/dev/null | python3 "$here/test-bins.py" >"$work/bins.tsv"
[ -s "$work/bins.tsv" ] || { echo "test-times: no test binaries found" >&2; exit 2; }

: >"$work/times.tsv"
while IFS=$'\t' read -r crate kind exe; do
  start=$(date +%s)
  # one test at a time: the time of a test is its own, not the time of whatever ran beside it
  # --format json: one event per test with its time; the plain format loses lines when a test's child process writes to stderr
  (cd "$src/crates/$crate" && RUSTC_BOOTSTRAP=1 CARGO_MANIFEST_DIR="$src/crates/$crate" "$exe" -Z unstable-options --format json \
    --report-time --test-threads=1 $mode 2>/dev/null) >"$work/run.json" || true
  python3 - "$crate" "$kind" "$work/run.json" >>"$work/times.tsv" <<'PY'
import json, sys
crate, kind, path = sys.argv[1:4]
for line in open(path, encoding="utf-8", errors="replace"):
    try:
        e = json.loads(line)
    except ValueError:
        continue
    if e.get("type") == "test" and e.get("event") in ("ok", "failed") and "exec_time" in e:
        print(f"{e['exec_time']:.3f}\t{crate}\t{kind}\t{e['name']}")
PY
  printf '%-14s %-22s %4ss\n' "$crate" "$kind" "$(( $(date +%s) - start ))"
done <"$work/bins.tsv"

sort -rn "$work/times.tsv" >"$work/sorted.tsv"
total=$(awk '{ s += $1 } END { printf "%.1f", s }' "$work/sorted.tsv")
count=$(wc -l <"$work/sorted.tsv" | tr -d ' ')
echo
echo "== the $top slowest of $count tests (total $total s, one at a time)"
head -n "$top" "$work/sorted.tsv" | awk -F'\t' '{ printf "%8.2f s  %s %s %s\n", $1, $2, $3, $4 }'
if [ -n "$out" ]; then mkdir -p "$out"; cp "$work/sorted.tsv" "$out/times.tsv"; fi
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### Slowest tests ${release:+(release) }${mode:+(ignored only) }— $count tests, $total s in total"
    echo "| seconds | crate | binary | test |"
    echo "|---:|---|---|---|"
    head -n "$top" "$work/sorted.tsv" | awk -F'\t' '{ printf "| %.2f | %s | %s | `%s` |\n", $1, $2, $3, $4 }'
  } >>"$GITHUB_STEP_SUMMARY"
fi

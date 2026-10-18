#!/usr/bin/env bash
# The benchmarks of crates/core/benches on CI: the human-readable report (p95 query latency at 500 and 5,000 apps, cold start, idle CPU)
# and the criterion benchmarks of the query path. Output goes to DIR (default: bench/) and, as Markdown, to the job summary.
#   scripts/ci/run-bench.sh [DIR]
# Never run on a developer's machine: benchmarks are performance work and run on CI only (docs/TESTING.md, docs/PERF.md).
set -euo pipefail
cd "$(dirname "$0")/../.."
out="${1:-bench}"
mkdir -p "$out"

echo "== report"
cargo bench -p ashdash-core --bench report 2>&1 | tee "$out/report.txt"
echo "== queries (criterion)"
cargo bench -p ashdash-core --bench queries -- --noplot 2>&1 | tee "$out/queries.txt"

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### Benchmarks on $(sw_vers -productName 2>/dev/null || uname -s) $(sw_vers -productVersion 2>/dev/null || true) ($(uname -m))"
    echo "#### report (crates/core/benches/report.rs)"
    echo '```'
    cat "$out/report.txt"
    echo '```'
    echo "#### queries (criterion)"
    echo '```'
    grep -E 'time:|^[A-Za-z].*/' "$out/queries.txt" | head -n 80
    echo '```'
  } >>"$GITHUB_STEP_SUMMARY"
fi

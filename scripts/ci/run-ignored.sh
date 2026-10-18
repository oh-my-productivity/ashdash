#!/usr/bin/env bash
# The Rust tests that do not run on a developer's machine: every `#[ignore = "ci: ..."]` test (real OS interaction, long waits, big
# data sets, absolute latency ceilings) in RELEASE mode, one at a time, plus the fuzz suites at their full case counts.
#   scripts/ci/run-ignored.sh [extra cargo test arguments, e.g. a test name filter]
# It is the `ignored` suite of stress.yml (scripts/ci-run.sh ignored). Do not run it on a developer's machine: it overwrites the clipboard,
# runs osascript and Finder automation and takes minutes (docs/TESTING.md).
set -euo pipefail
cd "$(dirname "$0")/../.."

# Tests that cannot run on a hosted runner, with the reason. They are skipped by name here and nowhere else, so the list stays visible.
skip=(
  # drives a TextEdit window with real key events; on a runner it waits for a window that never takes the focus and ends after about
  # eight minutes. The paste path is covered by the QA checklist (tests/e2e/qa/checklist.sh, 3.5.x).
  --skip paste_into_scratch_textedit_document
)

echo "== the #[ignore = \"ci: ...\"] tests, release mode, one at a time"
cargo test --release --workspace "$@" -- --ignored --test-threads=1 ${skip[@]+"${skip[@]}"}

echo "== the fuzz suites at their full case counts (ASHDASH_FUZZ_FULL=1)"
ASHDASH_FUZZ_FULL=1 cargo test --release -p ashdash-core --test fuzz

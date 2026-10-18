#!/usr/bin/env bash
# The local gate every change passes before review: format, lint, unit and integration tests, the fast script tests and the policy check.
# Only unit and integration tests run on a developer's machine; everything else (end-to-end, GUI, stress, performance, packaging) runs on CI:
# docs/TESTING.md. The gate therefore has a budget: it prints the time of every stage and fails when the total passes 180 s
# (the target on a laptop with a warm build cache is 90 s). A stage that grows the gate past that moves to CI.
#   scripts/check.sh
# CHECK_BUDGET_SECONDS can be raised on CI only (a cold build cache there is slower than a laptop); on a laptop it is refused.
set -euo pipefail
cd "$(dirname "$0")/.."

HARD_BUDGET=180
TARGET=90
budget=$HARD_BUDGET
if [ -n "${CHECK_BUDGET_SECONDS:-}" ]; then
  if [ "${GITHUB_ACTIONS:-}" = true ]; then
    budget=$CHECK_BUDGET_SECONDS
  else
    echo "check.sh: CHECK_BUDGET_SECONDS is honoured on CI only; the local budget is ${HARD_BUDGET} s (docs/TESTING.md)" >&2
    exit 2
  fi
fi

now_ms() { perl -MTime::HiRes=time -e 'printf "%d", time() * 1000'; }
names=(); times=()
t_start=$(now_ms)
elapsed_s() { echo $(( ($(now_ms) - t_start) / 1000 )); }

report() { # report: the table of stage times
  local i total_ms=$(( $(now_ms) - t_start ))
  echo
  echo "== stage times"
  for i in "${!names[@]}"; do printf '  %-8s %6.1f s\n' "${names[$i]}" "$(awk -v ms="${times[$i]}" 'BEGIN { print ms / 1000 }')"; done
  printf '  %-8s %6.1f s   (budget %s s, target %s s)\n' total "$(awk -v ms="$total_ms" 'BEGIN { print ms / 1000 }')" "$budget" "$TARGET"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
      echo "### scripts/check.sh stage times"
      echo "| stage | seconds |"
      echo "|---|---:|"
      for i in "${!names[@]}"; do printf '| %s | %s |\n' "${names[$i]}" "$(awk -v ms="${times[$i]}" 'BEGIN { printf "%.1f", ms / 1000 }')"; done
      printf '| **total** | **%s** (budget %s) |\n' "$(awk -v ms="$total_ms" 'BEGIN { printf "%.1f", ms / 1000 }')" "$budget"
    } >>"$GITHUB_STEP_SUMMARY"
  fi
}

over_budget() {
  report
  echo "check.sh: FAILED: the gate took longer than its ${budget} s budget." >&2
  echo "check.sh: the slowest stage above is where to look. Work that needs more time than this (long tests, stress, GUI, performance)" >&2
  echo "check.sh: is marked #[ignore = \"ci: ...\"] or moved to tests/e2e and runs on CI instead; see docs/TESTING.md." >&2
  exit 1
}

stage() { # stage <name> <command...>
  local name=$1 t0 rc=0; shift
  echo "== $name"
  t0=$(now_ms)
  "$@" || rc=$?
  names+=("$name"); times+=($(( $(now_ms) - t0 )))
  if [ "$rc" != 0 ]; then
    report
    echo "check.sh: FAILED in stage '$name' (exit status $rc)" >&2
    exit "$rc"
  fi
  if [ "$(elapsed_s)" -gt "$budget" ]; then over_budget; fi
}

stage fmt     cargo fmt --all -- --check
stage clippy  cargo clippy --workspace --all-targets -- -D warnings
stage test    cargo test --workspace
# the commit-message validator (about 6 s); the git hook tests (hooks.test.sh, about 40 s of real commits) run on CI: tests/hooks/run-tests.sh
stage commit-lint tests/hooks/commit-lint.test.sh
stage policy  scripts/policy-check.sh
stage policy-tests tests/policy/run-tests.sh

report
if [ "$(elapsed_s)" -gt "$budget" ]; then over_budget; fi
if [ "$(elapsed_s)" -gt "$TARGET" ]; then echo "check.sh: note: over the ${TARGET} s target (budget ${budget} s)"; fi
echo "check.sh: OK"

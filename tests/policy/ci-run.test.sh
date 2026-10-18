#!/usr/bin/env bash
# Tests of scripts/ci-run.sh with --dry-run and with refusals (no gh, no network): the suite -> workflow mapping, the inputs, the guard
# against main, and that every suite the script knows is a choice of the workflow it dispatches (so the two cannot drift apart).
# shellcheck disable=SC2015  # "test && ok || bad" is the reporting idiom here
# policy-allow-file: heavy -- the subject of this test is ci-run.sh, which is only ever run with --dry-run or with arguments it refuses
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
run="$repo/scripts/ci-run.sh"
fails=0
ok()  { printf '  ok   %s\n' "$1"; }
bad() { printf '  FAIL %s\n' "$1"; fails=$((fails + 1)); }
# expect_line <description> <expected line> <args...>: dry run succeeds and prints exactly that gh command
expect_line() {
  local d=$1 want=$2; shift 2
  local out; out=$("$run" "$@" --dry-run 2>&1); local rc=$?
  [ $rc = 0 ] && printf '%s\n' "$out" | grep -qxF "$want" && ok "$d" || bad "$d: rc=$rc want '$want' got: $out"
}
# refuses <description> <message fragment> <args...>
refuses() {
  local d=$1 frag=$2; shift 2
  local out; out=$("$run" "$@" 2>&1); local rc=$?
  [ $rc = 2 ] && printf '%s' "$out" | grep -qF -- "$frag" && ok "refuses $d" || bad "refuses $d: rc=$rc out: $out"
}

echo "== suites and inputs"
expect_line "ci" "gh workflow run ci.yml --ref feature/x" ci --ref feature/x
expect_line "smoke" "gh workflow run e2e.yml --ref feature/x -f suite=smoke" smoke --ref feature/x
expect_line "checklist with sections" "gh workflow run e2e.yml --ref feature/x -f suite=checklist -f sections=s31 s32" checklist --ref feature/x --sections "s31 s32"
expect_line "screenshots" "gh workflow run e2e.yml --ref feature/x -f suite=screenshots" screenshots --ref feature/x
expect_line "focus" "gh workflow run e2e.yml --ref feature/x -f suite=focus" focus --ref feature/x
expect_line "x86-64" "gh workflow run e2e.yml --ref feature/x -f suite=x86-64" x86-64 --ref feature/x
expect_line "bundle" "gh workflow run e2e.yml --ref feature/x -f suite=bundle" bundle --ref feature/x
expect_line "all of e2e" "gh workflow run e2e.yml --ref feature/x -f suite=all" e2e --ref feature/x
expect_line "ignored" "gh workflow run stress.yml --ref feature/x -f suite=ignored" ignored --ref feature/x
expect_line "timing with runs" "gh workflow run stress.yml --ref feature/x -f suite=timing -f runs=20 -f slow_runs=5" timing --ref feature/x --runs 20 --slow-runs 5
expect_line "perf-gui" "gh workflow run stress.yml --ref feature/x -f suite=perf-gui" perf-gui --ref feature/x
expect_line "bench" "gh workflow run stress.yml --ref feature/x -f suite=bench" bench --ref feature/x
expect_line "test-times" "gh workflow run stress.yml --ref feature/x -f suite=test-times" test-times --ref feature/x
expect_line "all of stress" "gh workflow run stress.yml --ref feature/x -f suite=all" stress --ref feature/x
expect_line "soak with a duration" "gh workflow run soak.yml --ref feature/x -f soak_seconds=600" soak --ref feature/x --soak-seconds 600
expect_line "a target" "gh workflow run stress.yml --ref feature/x -f suite=timing -f ref=0123456789abcdef0123456789abcdef01234567" timing --ref feature/x --target 0123456789abcdef0123456789abcdef01234567
expect_line "a free input" "gh workflow run e2e.yml --ref feature/x -f suite=smoke -f foo=bar" smoke --ref feature/x -f foo=bar
out=$("$run" smoke --ref feature/x --watch --download /tmp/out --dry-run 2>&1)
printf '%s' "$out" | grep -q "gh run watch" && printf '%s' "$out" | grep -q "gh run download <id> --dir /tmp/out" && ok "--watch and --download are described in the dry run" || bad "watch/download dry run: $out"
here_branch=$(git -C "$repo" symbolic-ref --quiet --short HEAD 2>/dev/null || true)
if [ -n "$here_branch" ] && [ "$here_branch" != main ] && [ "$here_branch" != master ]; then
  expect_line "the current branch is the default" "gh workflow run e2e.yml --ref $here_branch -f suite=smoke" smoke
fi

echo "== refusals"
# a throw-away repository on main: the guard looks at the branch you are on
tmp=$(mktemp -d "${TMPDIR:-/tmp}/ci-run-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/scripts"
cp "$run" "$tmp/scripts/ci-run.sh"
git -C "$tmp" init -q -b main
git -C "$tmp" -c user.name=t -c user.email=t@users.noreply.github.com commit -q --allow-empty -m "chore: init"
out=$("$tmp/scripts/ci-run.sh" smoke --dry-run 2>&1); rc=$?
[ $rc = 2 ] && printf '%s' "$out" | grep -q "with --ref main" && ok "refuses to run main when no --ref is given" || bad "main guard: rc=$rc $out"
out=$("$tmp/scripts/ci-run.sh" smoke --dry-run --ref main 2>&1); rc=$?
[ $rc = 0 ] && printf '%s' "$out" | grep -qxF "gh workflow run e2e.yml --ref main -f suite=smoke" && ok "runs main when --ref main is explicit" || bad "explicit main: rc=$rc $out"
git -C "$tmp" checkout -q --detach
out=$("$tmp/scripts/ci-run.sh" smoke --dry-run 2>&1); rc=$?
[ $rc = 2 ] && printf '%s' "$out" | grep -q "detached" && ok "refuses a detached HEAD without --ref" || bad "detached: rc=$rc $out"
refuses "an unknown suite" "unknown suite" nonsense --ref feature/x
refuses "no suite at all" "ci-run.sh <suite>" --ref feature/x
refuses "two suites" "one suite at a time" smoke soak --ref feature/x
refuses "an unknown option" "unknown option --frobnicate" smoke --frobnicate
refuses "a branch name with a space" "refusing the branch name" smoke --ref "a b" --dry-run
refuses "a branch name that looks like an option" "unknown option" smoke --ref=--evil
refuses "an input without a value" "needs name=value" smoke --ref feature/x -f novalue

echo "== the script and the workflows agree on the suites"
for pair in "e2e.yml:smoke checklist screenshots focus x86-64 bundle all" "stress.yml:ignored timing perf-gui bench test-times all"; do
  wf=${pair%%:*}; file="$repo/.github/workflows/$wf"
  [ -f "$file" ] || { bad "$wf is missing"; continue; }
  for s in ${pair#*:}; do
    grep -qE "^[[:space:]]+- \"?$s\"?[[:space:]]*$" "$file" && ok "$wf offers the suite '$s'" || bad "$wf does not offer the suite '$s' as a choice"
  done
done
for wf in ci.yml e2e.yml stress.yml soak.yml; do
  [ -f "$repo/.github/workflows/$wf" ] && grep -q "workflow_dispatch" "$repo/.github/workflows/$wf" && ok "$wf can be dispatched" || bad "$wf has no workflow_dispatch"
done

echo
if [ "$fails" = 0 ]; then echo "ci-run tests: OK"; else echo "ci-run tests: $fails FAILED"; exit 1; fi

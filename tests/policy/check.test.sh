#!/usr/bin/env bash
# Tests of scripts/check.sh itself, in a throw-away tree with stand-ins for cargo and for the scripts it runs: the stage table, the failure of
# a stage, the time budget (it fails with a clear message when the gate is too slow) and the rule that the budget can be raised on CI only.
# policy-allow-file: gui, network, heavy -- the stand-in scripts below are text; nothing in this file runs a real tool
# shellcheck disable=SC2015  # "test && ok || bad" is the reporting idiom here
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/check-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
fails=0
ok()  { printf '  ok   %s\n' "$1"; }
bad() { printf '  FAIL %s\n' "$1"; fails=$((fails + 1)); }

mkdir -p "$tmp/scripts" "$tmp/bin" "$tmp/tests/hooks" "$tmp/tests/policy"
cp "$repo/scripts/check.sh" "$tmp/scripts/check.sh"
# stand-ins: cargo succeeds (or fails for the subcommand named in FAKE_FAIL), the scripts print their name
printf '#!/usr/bin/env bash\n[ "${FAKE_FAIL:-}" = "$1" ] && { echo "fake cargo: $1 failed" >&2; exit 101; }\nexit 0\n' >"$tmp/bin/cargo"
for f in scripts/policy-check.sh tests/hooks/commit-lint.test.sh tests/policy/run-tests.sh; do
  printf '#!/usr/bin/env bash\necho "stand-in %s"\n' "$f" >"$tmp/$f"
done
chmod +x "$tmp/bin/cargo" "$tmp/scripts/"*.sh "$tmp/tests/hooks/"*.sh "$tmp/tests/policy/"*.sh

run() { # run <VAR=value>...: sets OUT and RC; PATH has the fake cargo first
  OUT=$(env -u GITHUB_ACTIONS -u CHECK_BUDGET_SECONDS "$@" PATH="$tmp/bin:$PATH" bash "$tmp/scripts/check.sh" 2>&1); RC=$?
}

run FAKE_X=1
[ $RC = 0 ] && printf '%s' "$OUT" | grep -q "check.sh: OK" && ok "a fast gate passes" || bad "fast gate: rc=$RC $OUT"
for st in fmt clippy test commit-lint policy policy-tests; do
  printf '%s' "$OUT" | grep -Eq "^  $st +[0-9.]+ s$" && ok "the stage table lists $st with its time" || bad "no time for $st: $OUT"
done
printf '%s' "$OUT" | grep -Eq "^  total +[0-9.]+ s +\(budget 180 s, target 90 s\)" && ok "the table ends with the total and the budget" || bad "total line: $OUT"

run FAKE_FAIL=clippy
[ $RC = 101 ] && printf '%s' "$OUT" | grep -q "FAILED in stage 'clippy'" && ! printf '%s' "$OUT" | grep -q "stand-in" && ok "a failing stage stops the gate and is named" || bad "failing stage: rc=$RC $OUT"

run CHECK_BUDGET_SECONDS=10 FAKE_X=1
[ $RC = 2 ] && printf '%s' "$OUT" | grep -q "honoured on CI only" && ok "a laptop cannot raise the budget" || bad "budget override on a laptop: rc=$RC $OUT"

run GITHUB_ACTIONS=true CHECK_BUDGET_SECONDS=900 FAKE_X=1
[ $RC = 0 ] && printf '%s' "$OUT" | grep -q "budget 900 s" && ok "CI can raise the budget (shown in the table)" || bad "budget on CI: rc=$RC $OUT"

# a budget of zero seconds: the first stage already takes longer than that
run GITHUB_ACTIONS=true CHECK_BUDGET_SECONDS=0 FAKE_X=1
[ $RC = 1 ] && printf '%s' "$OUT" | grep -q "took longer than its 0 s budget" && printf '%s' "$OUT" | grep -q "docs/TESTING.md" && ok "a gate over its budget fails with a clear message" || bad "over budget: rc=$RC $OUT"
printf '%s' "$OUT" | grep -q "== stage times" && ok "the stage times are printed when the budget is exceeded" || bad "no table when over budget: $OUT"

echo
if [ "$fails" = 0 ]; then echo "check.sh tests: OK"; else echo "check.sh tests: $fails FAILED"; exit 1; fi

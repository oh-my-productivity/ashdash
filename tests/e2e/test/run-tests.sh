#!/usr/bin/env bash
# Host-side tests of the end-to-end scripts that can be tested without a GUI: the guards of tcc-grant.sh and of the CI wrappers (they must
# refuse outside a hosted runner), finalize.py and retry-sections.py. Fake tools stand in for sysctl, sudo, sqlite3 ...; nothing real is
# touched. CI only (job `e2e-scripts` of e2e.yml): the scripts are CI-only, so their tests are too (docs/TESTING.md).
# Usage: tests/e2e/test/run-tests.sh
# shellcheck disable=SC2015  # "test && ok || bad" is the reporting idiom here
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
e2e=$(cd "$here/.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/e2e-test.XXXXXX")
fails=0
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT

ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fails=$((fails + 1)); }
check() { # check <description> <command...>
  local d=$1; shift
  if "$@" >/dev/null 2>&1; then ok "$d"; else bad "$d"; fi
}

echo "== guest scripts"
for f in tcc-grant.sh gui-exec; do check "guest/$f parses" bash -n "$e2e/guest/$f"; done
for f in smoke.sh qa/checklist.sh qa/soak.sh qa/prep.sh qa/pkg-install.sh focus/probe.sh focus/regress.sh; do check "$f parses" bash -n "$e2e/$f"; done

# tcc-grant.sh must not write a single TCC row outside a hosted runner, and must check its tools before the first write.
# The tools are faked through PATH; the real TCC databases and tccd are never touched.
tg="$tmp/tcc"; mkdir -p "$tg/bin" "$tg/home"
tg_fake() { printf '#!/bin/sh\n%s\n' "$2" >"$tg/bin/$1"; chmod +x "$tg/bin/$1"; }
tg_fake sysctl 'echo "${FAKE_HW_MODEL:-VirtualMac2,1}"'
tg_fake id 'echo "${FAKE_USER:-runner}"'
tg_fake csrutil 'echo "${FAKE_SIP:-System Integrity Protection status: disabled.}"'
tg_fake sudo 'if [ "${FAKE_NO_SUDO:-0}" = 1 ]; then exit 1; fi; [ "$1" = -n ] && shift; if [ "$1" = true ]; then exit 0; fi; echo "sudo $*" >>"$FAKE_LOG"; "$@"'
tg_fake sqlite3 'echo "sqlite3 $*" >>"$FAKE_LOG"'
tg_fake python3 'if [ "${FAKE_NO_PYTHON:-0}" = 1 ]; then exit 127; fi; cat >/dev/null 2>&1; echo "python3 $*" >>"$FAKE_LOG"'
tg_fake killall 'echo "killall $*" >>"$FAKE_LOG"'
tg_fake sleep 'exit 0'
# A hosted runner is a VirtualMac with the account "runner" and both variables set. Everything must agree: the variables alone (exported
# on a developer's Mac, or set by a self-hosted runner) never open the door.
CI_ENV="GITHUB_ACTIONS=true RUNNER_ENVIRONMENT=github-hosted"
tg_run() { # tg_run <VAR=value>...: runs the script with the fakes first in PATH; the log shows what it wrote
  : >"$tg/log"
  env -u GITHUB_ACTIONS -u RUNNER_ENVIRONMENT "$@" FAKE_LOG="$tg/log" HOME="$tg/home" PATH="$tg/bin:/usr/bin:/bin" bash "$e2e/guest/tcc-grant.sh" >"$tg/out" 2>&1
}
tg_refused() { # tg_refused <description> <VAR=value>...
  local d=$1; shift
  tg_run "$@"; local rc=$?
  [ $rc != 0 ] && [ ! -s "$tg/log" ] && ok "tcc-grant.sh refuses $d, nothing written" || bad "tcc-grant.sh $d: rc=$rc log=$(head -c 200 "$tg/log") out=$(head -c 200 "$tg/out")"
}
# shellcheck disable=SC2086  # CI_ENV is a list of VAR=value words
tg_refused "on a real Mac even with the CI variables set" $CI_ENV FAKE_HW_MODEL=Mac15,6
grep -q "not a virtual machine" "$tg/out" && ok "the message names the reason" || bad "message: $(cat "$tg/out")"
tg_refused "outside CI (no variables)" FAKE_X=1
tg_refused "outside CI in a VM as admin" FAKE_USER=admin
# shellcheck disable=SC2086
tg_refused "in CI as another account than runner" $CI_ENV FAKE_USER=admin
tg_refused "on a self-hosted runner" GITHUB_ACTIONS=true RUNNER_ENVIRONMENT=self-hosted
tg_refused "as runner without GITHUB_ACTIONS" RUNNER_ENVIRONMENT=github-hosted
tg_refused "as runner outside CI" FAKE_USER=runner
# shellcheck disable=SC2086
tg_refused "in CI with SIP on" $CI_ENV "FAKE_SIP=System Integrity Protection status: enabled."
# shellcheck disable=SC2086
tg_refused "in CI with a partial SIP configuration" $CI_ENV "FAKE_SIP=System Integrity Protection status: unknown (Custom Configuration)."
# shellcheck disable=SC2086
tg_refused "in CI without passwordless sudo" $CI_ENV FAKE_NO_SUDO=1
# shellcheck disable=SC2086
tg_refused "in CI without a working python3" $CI_ENV FAKE_NO_PYTHON=1
# shellcheck disable=SC2086
tg_run $CI_ENV; rc=$?
rows=$(grep -c '^sudo sqlite3\|^sqlite3' "$tg/log" || true)
[ $rc = 0 ] && [ "$rows" -ge 20 ] && grep -q '^killall tccd' "$tg/log" && grep -q 'hosted runner' "$tg/out" && ok "on a hosted runner (VirtualMac, user runner, SIP off, sudo) it grants ($rows sqlite3 calls)" || bad "CI run: rc=$rc rows=$rows out=$(tail -3 "$tg/out")"

echo "== CI wrappers (tests/e2e/ci)"
ci="$e2e/ci"
for f in common.sh run-smoke.sh run-checklist.sh run-soak.sh run-screenshots.sh run-focus.sh run-perf-gui.sh verify-bundle.sh; do check "ci/$f parses" bash -n "$ci/$f"; done
# They drive the real keyboard and screen: outside a hosted runner they must refuse before doing anything.
# (On a real runner these variables are set, so the tests clear them; the "spoofed" case fakes the account instead.)
for w in run-smoke.sh run-checklist.sh run-soak.sh; do
  env -u GITHUB_ACTIONS -u RUNNER_ENVIRONMENT bash "$ci/$w" >"$tmp/ciw.out" 2>&1; rc=$?
  [ $rc != 0 ] && grep -q "refusing" "$tmp/ciw.out" && ok "$w refuses outside GitHub Actions" || bad "$w outside CI: rc=$rc $(head -c 200 "$tmp/ciw.out")"
  env -u RUNNER_ENVIRONMENT GITHUB_ACTIONS=true bash "$ci/$w" >"$tmp/ciw.out" 2>&1; rc=$?
  [ $rc != 0 ] && grep -q "refusing" "$tmp/ciw.out" && ok "$w refuses on a self-hosted runner" || bad "$w self-hosted: rc=$rc"
  tg_fake id 'echo someone'
  env FAKE_LOG="$tg/log" PATH="$tg/bin:$PATH" GITHUB_ACTIONS=true RUNNER_ENVIRONMENT=github-hosted bash "$ci/$w" >"$tmp/ciw.out" 2>&1; rc=$?
  [ $rc != 0 ] && grep -q "refusing" "$tmp/ciw.out" && ok "$w refuses for another account than runner even with the CI variables" || bad "$w spoofed: rc=$rc"
done
rm -f "$tg/bin/id"
# finalize.py: FAIL becomes BLOCKED only for listed rows (and images), anything else FAILs the job; missing required rows fail too.
fin="$tmp/fin"; mkdir -p "$fin"
printf 'SETUP\tPASS\tok\n1.1\tFAIL\tbroken\n2.2\tFAIL\tsilly\n3.3\tPASS\tgood\n2.2\tFAIL\tsilly again\n' >"$fin/r.tsv"
printf '# comment\n2.2\tbecause\n4.4\tother image only\tmacos26\n' >"$fin/b.tsv"
CI_IMAGE=macos15 python3 "$ci/finalize.py" "$fin/r.tsv" "$fin/b.tsv" "$fin/o.tsv" "$fin/o.md" --require SETUP >/dev/null 2>&1; rc=$?
[ $rc = 1 ] && grep -q '^2.2.BLOCKED.\[hosted runner: because\]' "$fin/o.tsv" && grep -q '^1.1.FAIL' "$fin/o.tsv" && ok "finalize: listed FAIL becomes BLOCKED, an unlisted FAIL fails the job" || bad "finalize: rc=$rc $(cat "$fin/o.tsv")"
printf 'SETUP\tPASS\tok\n2.2\tFAIL\tsilly\n4.4\tFAIL\tx\n' >"$fin/r2.tsv"
CI_IMAGE=macos15 python3 "$ci/finalize.py" "$fin/r2.tsv" "$fin/b.tsv" "$fin/o2.tsv" "$fin/o2.md" >/dev/null 2>&1; rc=$?
[ $rc = 1 ] && grep -q '^4.4.FAIL' "$fin/o2.tsv" && ok "finalize: an image-limited rule does not apply to another image" || bad "finalize image rule: rc=$rc"
CI_IMAGE=macos26 python3 "$ci/finalize.py" "$fin/r2.tsv" "$fin/b.tsv" "$fin/o3.tsv" "$fin/o3.md" >/dev/null 2>&1; rc=$?
[ $rc = 0 ] && ok "finalize: all FAILs listed for this image: job passes, BLOCKED rows are kept visible" || bad "finalize all listed: rc=$rc"
printf 'SETUP\tFAIL\tno\n' >"$fin/r4.tsv"
python3 "$ci/finalize.py" "$fin/r4.tsv" "$fin/b.tsv" "$fin/o4.tsv" "$fin/o4.md" --require SETUP >/dev/null 2>&1; rc=$?
[ $rc = 1 ] && ok "finalize: a required row that is not PASS fails" || bad "finalize required: rc=$rc"
python3 "$ci/finalize.py" "$fin/none.tsv" "$fin/b.tsv" "$fin/o5.tsv" "$fin/o5.md" >/dev/null 2>&1; rc=$?
[ $rc = 1 ] && ok "finalize: no result file at all fails" || bad "finalize empty: rc=$rc"

printf 's_setup() {\n  result SETUP PASS x\n}\ns31() {\n  result 3.1.1a PASS x; result 3.1.1b FAIL y\n}\ns32() {\n  result 3.2.1a FAIL z\n}\nw45() {\n  result W4.1 FAIL q\n}\n' >"$fin/cl.sh"
printf '3.1.1b\tFAIL\ty\n3.2.1a\tPASS\tz\nW4.1\tFAIL\tq\n' >"$fin/rf.tsv"
[ "$(python3 "$ci/retry-sections.py" "$fin/cl.sh" "$fin/rf.tsv" | tr '\n' ' ')" = "s31 w45 " ] && ok "retry-sections: maps FAILed rows to the sections that produce them" || bad "retry-sections: $(python3 "$ci/retry-sections.py" "$fin/cl.sh" "$fin/rf.tsv" | tr '\n' ' ')"


echo
if [ "$fails" = 0 ]; then echo "e2e script tests: OK"; else echo "e2e script tests: $fails FAILED"; exit 1; fi

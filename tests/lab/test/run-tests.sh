#!/usr/bin/env bash
# Host-side tests of the lab scripts against a fake `tart` and a fake `ssh` (no VM needed, ~40 s).
# Usage: tests/lab/test/run-tests.sh
# shellcheck disable=SC2015  # "test && ok || bad" is the reporting idiom here
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
lab=$(cd "$here/.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/lab-test.XXXXXX")
fails=0
cleanup() { pkill -f "$tmp" 2>/dev/null || true; rm -rf "$tmp"; }
trap cleanup EXIT

export TART_HOME="$tmp/tart"
export TART_BIN="$here/fake-tart"
export LAB_RUN_DIR="$tmp/run"
export LAB_STATE_DIR="$tmp/state"
export LAB_KEY="$tmp/key"
export LAB_REAL_HOME="$tmp/realhome"
export FAKE_GUEST_HOME="$tmp/guest"
export PATH="$here/bin:$PATH"
export LAB_MAX_VM_AGE=1800
mkdir -p "$TART_HOME/vms/ashdash-golden" "$LAB_STATE_DIR" "$LAB_REAL_HOME" "$FAKE_GUEST_HOME"
echo stopped >"$TART_HOME/vms/ashdash-golden/state"
: >"$LAB_KEY"

ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fails=$((fails + 1)); }
check() { # check <description> <command...>
  local d=$1; shift
  if "$@" >/dev/null 2>&1; then ok "$d"; else bad "$d"; fi
}
run_vms() { "$TART_BIN" list --source local --quiet | grep -c '^ashdash-run-' || true; }
slots() { python3 "$lab/slot.py" --run-dir "$LAB_RUN_DIR" "$@"; }

echo "== slot semaphore"
slots acquire --name a --timeout 5 >/dev/null; check "first slot acquired" slots holds --name a
slots acquire --name b --timeout 5 >/dev/null; check "second slot acquired" slots holds --name b
slots acquire --name c --timeout 2 >/dev/null 2>&1; rc=$?
[ "$rc" = 3 ] && ok "third acquire times out (exit 3) instead of taking a slot" || bad "third acquire rc=$rc"
slots release --name a; check "release frees slot a" bash -c "! python3 '$lab/slot.py' --run-dir '$LAB_RUN_DIR' holds --name a"
slots acquire --name c --timeout 5 >/dev/null; check "queued job gets the freed slot" slots holds --name c
# stale recovery: SIGKILL the holder, the kernel drops the flock
pid=$(python3 - "$LAB_RUN_DIR" <<'PY'
import glob, json, sys
for p in glob.glob(sys.argv[1] + "/slots/*.lock"):
    d = open(p).read().strip()
    if d and json.loads(d).get("name") == "b":
        print(json.loads(d)["pid"])
PY
)
kill -9 "$pid"; sleep 0.3
slots acquire --name d --timeout 5 >/dev/null; check "slot of a SIGKILLed holder is recovered" slots holds --name d
slots release --name c; slots release --name d

echo "== lab-with-vm basics"
out=$("$lab/lab-with-vm" -- uname -s 2>"$tmp/uname.err"); rc=$?
[ $rc = 0 ] || sed 's/^/    | /' "$tmp/uname.err"
[ $rc = 0 ] && [ "$out" = "$(uname -s)" ] && ok "runs the command ('$out') and returns 0" || bad "uname: rc=$rc out=$out"
[ "$(run_vms)" = 0 ] && ok "no ashdash-run-* VM left" || bad "VM left behind"
[ -d "$TART_HOME/vms/ashdash-golden" ] && ok "golden image untouched" || bad "golden image gone"
"$lab/lab-with-vm" -- sh -c 'exit 7' >/dev/null 2>&1; rc=$?
[ $rc = 7 ] && ok "propagates exit code 7" || bad "exit code $rc"
[ "$(run_vms)" = 0 ] && ok "cleanup after failing command" || bad "VM left after failure"
FAKE_TART_CLONE_FAIL=1 "$lab/lab-with-vm" -- true >/dev/null 2>&1; rc=$?
[ $rc != 0 ] && ok "clone failure is loud (rc=$rc)" || bad "clone failure ignored"
[ "$(run_vms)" = 0 ] && ! slots status | grep -qv free && ok "clone failure leaves no VM and no slot" || bad "leak after clone failure: $(slots status)"

s0=$(date +%s)
FAKE_TART_RUN_FAIL=1 "$lab/lab-with-vm" -- true >"$tmp/runfail.out" 2>"$tmp/runfail.err"; rc=$?
[ $rc != 0 ] && grep -q "exited early" "$tmp/runfail.err" && [ $(( $(date +%s) - s0 )) -lt 30 ] && ok "a VM that fails to boot fails fast with the tart log" || bad "boot failure: rc=$rc $(tail -2 "$tmp/runfail.err")"
[ "$(run_vms)" = 0 ] && slots status | grep -qx 'slot0: free, slot1: free' && ok "boot failure leaves no VM and no slot" || bad "leak after boot failure: $(slots status)"

echo "== cleanup trap on signals"
"$lab/lab-with-vm" --id sigtest -- sleep 60 >/dev/null 2>&1 &
jp=$!
for _ in $(seq 1 30); do [ "$(run_vms)" = 1 ] && break; sleep 0.5; done
sleep 1
kill -TERM "$jp"; wait "$jp" 2>/dev/null
sleep 1
[ "$(run_vms)" = 0 ] && ok "SIGTERM mid-run deletes the VM" || bad "VM left after SIGTERM"
check "SIGTERM mid-run frees the slot" bash -c "! python3 '$lab/slot.py' --run-dir '$LAB_RUN_DIR' holds --name ashdash-run-sigtest"

echo "== artifacts and sync"
src="$tmp/src"; mkdir -p "$src/target" "$src/.worktrees/x" "$src/keep" "$src/.git"
echo hi >"$src/keep/a.txt"; echo big >"$src/target/b.bin"; echo w >"$src/.worktrees/x/c"; echo g >"$src/.git/HEAD"
printf 'ignored.log\n' >"$src/.gitignore"; echo l >"$src/ignored.log"
# shellcheck disable=SC2016  # expanded in the (fake) guest
"$lab/lab-with-vm" --src "$src" --artifacts "$tmp/art" -- sh -c 'echo shot >"$LAB_ARTIFACTS/s.txt"; ls -A >"$LAB_ARTIFACTS/ls.txt"' >/dev/null 2>&1; rc=$?
[ $rc = 0 ] && ok "job with --src/--artifacts succeeds" || bad "rc=$rc"
[ -f "$tmp/art/s.txt" ] && ok "artifact copied out" || bad "artifact missing"
ls_out=$(cat "$tmp/art/ls.txt" 2>/dev/null)
echo "$ls_out" | grep -q '^keep$' && ok "worktree files synced" || bad "keep/ not synced: $ls_out"
echo "$ls_out" | grep -Eq '^(target|\.worktrees|\.git|ignored\.log)$' && bad "excluded paths were synced: $ls_out" || ok "target/, .worktrees/, .git and .gitignore'd files are not synced"

# --no-sync on a VM that never had a sync: the work dir must still exist (it used to fail with "cd: no such file or directory").
# shellcheck disable=SC2016  # expanded in the (fake) guest
"$lab/lab-with-vm" --src "$tmp/src" --no-sync -- sh -c 'pwd >"$HOME/nosync.pwd"' >/dev/null 2>&1; rc=$?
[ $rc = 0 ] && [ -s "$FAKE_GUEST_HOME/nosync.pwd" ] && ok "--no-sync on a fresh VM still runs (work dir is created)" || bad "--no-sync on a fresh VM: rc=$rc"

echo "== guest scripts"
for f in tcc-grant.sh check-golden.sh tcc-probe.sh provision.sh; do check "guest/$f parses" bash -n "$lab/guest/$f"; done
check "smoke.sh parses" bash -n "$lab/smoke.sh"

# tcc-grant.sh must not write a single TCC row outside a lab VM, and must check its tools before the first write.
# The tools are faked through PATH; the real TCC databases and tccd are never touched.
tg="$tmp/tcc"; mkdir -p "$tg/bin" "$tg/home"
tg_fake() { printf '#!/bin/sh\n%s\n' "$2" >"$tg/bin/$1"; chmod +x "$tg/bin/$1"; }
tg_fake sysctl 'echo "${FAKE_HW_MODEL:-VirtualMac2,1}"'
tg_fake id 'echo "${FAKE_USER:-admin}"'
tg_fake csrutil 'echo "${FAKE_SIP:-System Integrity Protection status: disabled.}"'
tg_fake sudo 'if [ "${FAKE_NO_SUDO:-0}" = 1 ]; then exit 1; fi; [ "$1" = -n ] && shift; if [ "$1" = true ]; then exit 0; fi; echo "sudo $*" >>"$FAKE_LOG"; "$@"'
tg_fake sqlite3 'echo "sqlite3 $*" >>"$FAKE_LOG"'
tg_fake python3 'if [ "${FAKE_NO_PYTHON:-0}" = 1 ]; then exit 127; fi; cat >/dev/null 2>&1; echo "python3 $*" >>"$FAKE_LOG"'
tg_fake killall 'echo "killall $*" >>"$FAKE_LOG"'
tg_fake sleep 'exit 0'
tg_run() { # tg_run <VAR=value>...: runs the script with the fakes first in PATH; the log shows what it wrote
  : >"$tg/log"
  env -u GITHUB_ACTIONS -u RUNNER_ENVIRONMENT "$@" FAKE_LOG="$tg/log" HOME="$tg/home" PATH="$tg/bin:/usr/bin:/bin" bash "$lab/guest/tcc-grant.sh" >"$tg/out" 2>&1
}
tg_refused() { # tg_refused <description> <VAR=value>...
  local d=$1; shift
  tg_run "$@"; local rc=$?
  [ $rc != 0 ] && [ ! -s "$tg/log" ] && ok "tcc-grant.sh refuses $d, nothing written" || bad "tcc-grant.sh $d: rc=$rc log=$(head -c 200 "$tg/log") out=$(head -c 200 "$tg/out")"
}
tg_refused "on a real Mac (hw.model Mac15,6)" FAKE_HW_MODEL=Mac15,6
grep -q "not a lab VM" "$tg/out" && ok "the message names the reason" || bad "message: $(cat "$tg/out")"
tg_refused "for another user than admin" FAKE_USER=someone
tg_refused "with SIP on" "FAKE_SIP=System Integrity Protection status: enabled."
tg_refused "with a partial SIP configuration" "FAKE_SIP=System Integrity Protection status: unknown (Custom Configuration)."
tg_refused "without passwordless sudo" FAKE_NO_SUDO=1
tg_refused "without a working python3" FAKE_NO_PYTHON=1
tg_run FAKE_X=1; rc=$?
rows=$(grep -c '^sudo sqlite3\|^sqlite3' "$tg/log" || true)
[ $rc = 0 ] && [ "$rows" -ge 20 ] && grep -q '^killall tccd' "$tg/log" && ok "in a VM as admin with sudo and python3 it grants ($rows sqlite3 calls)" || bad "VM run: rc=$rc rows=$rows out=$(tail -3 "$tg/out")"
# CI mode: a GitHub-hosted runner is a VirtualMac too, with the account "runner". Both variables, the VM model and the account must agree;
# the variables alone (exported on a developer's Mac, or set by a self-hosted runner) never open the door.
CI_ENV="GITHUB_ACTIONS=true RUNNER_ENVIRONMENT=github-hosted"
# shellcheck disable=SC2086  # CI_ENV is a list of VAR=value words
tg_refused "on a real Mac even with the CI variables set" $CI_ENV FAKE_HW_MODEL=Mac15,6 FAKE_USER=runner
# shellcheck disable=SC2086
tg_refused "in CI as the lab account admin" $CI_ENV FAKE_USER=admin
tg_refused "on a self-hosted runner" GITHUB_ACTIONS=true RUNNER_ENVIRONMENT=self-hosted FAKE_USER=runner
tg_refused "as runner without GITHUB_ACTIONS" RUNNER_ENVIRONMENT=github-hosted FAKE_USER=runner
tg_refused "as runner outside CI" FAKE_USER=runner
# shellcheck disable=SC2086
tg_refused "in CI with SIP on" $CI_ENV FAKE_USER=runner "FAKE_SIP=System Integrity Protection status: enabled."
# shellcheck disable=SC2086
tg_refused "in CI without passwordless sudo" $CI_ENV FAKE_USER=runner FAKE_NO_SUDO=1
# shellcheck disable=SC2086
tg_run $CI_ENV FAKE_USER=runner; rc=$?
rows=$(grep -c '^sudo sqlite3\|^sqlite3' "$tg/log" || true)
[ $rc = 0 ] && [ "$rows" -ge 20 ] && grep -q '^killall tccd' "$tg/log" && grep -q 'hosted runner' "$tg/out" && ok "on a hosted runner (VirtualMac, user runner, SIP off, sudo) it grants ($rows sqlite3 calls)" || bad "CI run: rc=$rc rows=$rows out=$(tail -3 "$tg/out")"
# check-golden must treat a timed-out System Events call as NOT granted (it used to report "look granted")
fake_guest="$tmp/fakeguest"; mkdir -p "$fake_guest/bin"
cp "$lab/guest/check-golden.sh" "$fake_guest/check-golden.sh"
printf '#!/bin/sh\necho "gui-exec: timed out after 30s" >&2; exit 124\n' >"$fake_guest/gui-exec"; chmod +x "$fake_guest/gui-exec"
out=$(PATH="$fake_guest/bin:$PATH" bash "$fake_guest/check-golden.sh" 2>&1 | grep -F "=> TCC")
echo "$out" | grep -q "NOT GRANTED" && ok "a timed-out System Events call is reported as NOT GRANTED" || bad "check-golden verdict: $out"

echo "== CI wrappers (tests/lab/ci)"
ci="$lab/ci"
for f in common.sh run-smoke.sh run-checklist.sh run-soak.sh verify-bundle.sh lint.sh; do check "ci/$f parses" bash -n "$ci/$f"; done
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

echo "== idempotence and naming"
"$lab/lab-down" ashdash-run-doesnotexist >/dev/null 2>&1 && ok "lab-down of a missing VM succeeds" || bad "lab-down not idempotent"
"$lab/lab-down" ashdash-golden >/dev/null 2>&1 && bad "lab-down accepted the golden image" || ok "lab-down refuses non-run names"
"$lab/lab-run" ashdash-golden -- true >/dev/null 2>&1 && bad "lab-run accepted the golden image" || ok "lab-run refuses non-run names"
r1=$("$lab/lab-up" --id idem 2>/dev/null); r2=$("$lab/lab-up" --id idem 2>/dev/null)
[ "$r1" = "$r2" ] && [ -n "$r1" ] && ok "lab-up twice with one id reuses the VM ($r1)" || bad "lab-up not idempotent: '$r1' vs '$r2'"
"$lab/lab-down" ashdash-run-idem >/dev/null 2>&1; "$lab/lab-down" ashdash-run-idem >/dev/null 2>&1 && ok "lab-down twice" || bad "second lab-down failed"
[ "$(run_vms)" = 0 ] && ok "nothing left" || bad "VM left"

echo "== watchdog"
mkdir -p "$TART_HOME/vms/ashdash-run-old"; echo stopped >"$TART_HOME/vms/ashdash-run-old/state"
mkdir -p "$TART_HOME/vms/other-vm"; echo stopped >"$TART_HOME/vms/other-vm/state"
sleep 2
"$lab/lab-gc" --max-age 1 >/dev/null 2>&1
[ ! -d "$TART_HOME/vms/ashdash-run-old" ] && ok "watchdog deleted the stale run VM" || bad "stale VM kept"
[ -d "$TART_HOME/vms/other-vm" ] && [ -d "$TART_HOME/vms/ashdash-golden" ] && ok "watchdog left unrelated VMs alone" || bad "watchdog deleted a foreign VM"

echo "== watchdog respects lab-up --max-age"
# A long run (soak) must survive the watchdog that other agents' lab-up calls run with the default 30 minutes.
vm=$("$lab/lab-up" --id longrun --max-age 100000 2>/dev/null | cut -d' ' -f1)
[ "$(cat "$LAB_RUN_DIR/max-age/ashdash-run-longrun" 2>/dev/null)" = 100000 ] && ok "lab-up --max-age records the limit" || bad "no max-age marker"
sleep 2
"$lab/lab-gc" --max-age 1000000 >/dev/null 2>&1; LAB_MAX_VM_AGE=1 "$lab/lab-gc" >/dev/null 2>&1
[ -d "$TART_HOME/vms/ashdash-run-longrun" ] && ok "the default watchdog (even with a 1 s default) keeps a VM that asked for more" || bad "long-run VM was reaped"
"$lab/lab-gc" --max-age 1 >/dev/null 2>&1
[ ! -d "$TART_HOME/vms/ashdash-run-longrun" ] && ok "an explicit lab-gc --max-age still wins" || { bad "explicit --max-age ignored"; "$lab/lab-down" ashdash-run-longrun >/dev/null 2>&1; }
[ ! -e "$LAB_RUN_DIR/max-age/ashdash-run-longrun" ] && ok "lab-down removes the marker" || bad "marker left"
[ -n "$vm" ] || bad "lab-up printed no VM name"

echo "== concurrency: 2 slots, third job queues"
for n in 1 2 3; do
  ( "$lab/lab-with-vm" --id "conc$n" -- sh -c 'date +%s; sleep 6' >"$tmp/conc$n.out" 2>/dev/null ) &
  sleep 0.7
done
sleep 3
running=$(run_vms)
[ "$running" = 2 ] && ok "while 3 jobs are submitted only 2 VMs exist" || bad "VMs running: $running"
wait
t=()
for n in 1 2 3; do t+=("$(head -1 "$tmp/conc$n.out" 2>/dev/null)"); done
if [ -n "${t[2]}" ] && [ "${t[2]}" -ge "$(( (t[0] < t[1] ? t[0] : t[1]) + 5 ))" ]; then ok "third job started only after a slot freed (${t[*]})"; else bad "third job did not queue: ${t[*]}"; fi
[ "$(run_vms)" = 0 ] && ok "all VMs cleaned up" || bad "VMs left over"

echo "== base image lookup (OCI images are not 'local')"
base_dir="$TART_HOME/oci/ghcr.io%cirruslabs%macos-sequoia-base:latest"
export LAB_BASE_IMAGE=ghcr.io/cirruslabs/macos-sequoia-base:latest
out=$("$lab/golden-setup.sh" create 2>&1); rc=$?
[ $rc != 0 ] && echo "$out" | grep -q "is not pulled yet" && ok "create refuses when the base image is not pulled" || bad "create without base: rc=$rc"
mkdir -p "$base_dir"
out=$(bash -c ". '$lab/lib.sh'; lab_image_exists '$LAB_BASE_IMAGE' && ! lab_vm_exists '$LAB_BASE_IMAGE' && echo both" 2>&1)
[ "$out" = both ] && ok "a pulled OCI image counts for lab_image_exists but is not a local VM" || bad "image lookup: $out"
# create must get past the pulled-check (it then refuses because ashdash-golden exists and no --force)
out=$("$lab/golden-setup.sh" create 2>&1); rc=$?
[ $rc != 0 ] && echo "$out" | grep -q "use --force" && ok "create finds the pulled base image" || bad "create with pulled base: rc=$rc out=$out"
rm -rf "$base_dir"

echo
if [ "$fails" = 0 ]; then echo "lab tests: OK"; else echo "lab tests: $fails FAILED"; exit 1; fi

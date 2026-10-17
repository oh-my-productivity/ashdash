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
# check-golden must treat a timed-out System Events call as NOT granted (it used to report "look granted")
fake_guest="$tmp/fakeguest"; mkdir -p "$fake_guest/bin"
cp "$lab/guest/check-golden.sh" "$fake_guest/check-golden.sh"
printf '#!/bin/sh\necho "gui-exec: timed out after 30s" >&2; exit 124\n' >"$fake_guest/gui-exec"; chmod +x "$fake_guest/gui-exec"
out=$(PATH="$fake_guest/bin:$PATH" bash "$fake_guest/check-golden.sh" 2>&1 | grep -F "=> TCC")
echo "$out" | grep -q "NOT GRANTED" && ok "a timed-out System Events call is reported as NOT GRANTED" || bad "check-golden verdict: $out"

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

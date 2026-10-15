#!/usr/bin/env bash
# accept.sh - acceptance run of the lab against REAL tart VMs (needs ashdash-golden; takes a few minutes).
#   1. lab-with-vm -- uname -a against a real clone (timed: clone+boot+ssh, command, teardown)
#   2. three concurrent lab-with-vm jobs: two run, the third queues (start times compared)
#   3. cleanup: no ashdash-run-* VM and no held slot afterwards; golden untouched
#   4. disk: size of the golden image and what a clone costs
# Prints a Markdown-ish report on stdout; exit 1 on any failed check.
# shellcheck disable=SC2015  # "test && ok || bad" is the reporting idiom
set -uo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"
lab_need_tart
fails=0
ok()  { printf 'ok   %s\n' "$1"; }
bad() { printf 'FAIL %s\n' "$1"; fails=$((fails + 1)); }
now() { python3 -c 'import time; print(time.time())'; }
run_vms() { "$TART_BIN" list --source local --quiet | grep -c "^${LAB_PREFIX}" || true; }
tmp=$(mktemp -d "${TMPDIR:-/tmp}/lab-accept.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

lab_vm_exists "$LAB_GOLDEN" || { echo "FAIL: $LAB_GOLDEN missing"; exit 1; }
echo "golden: $("$TART_BIN" list --source local | grep -F "$LAB_GOLDEN" | tr -s ' ')"

echo "== 1. uname -a in a real clone"
t0=$(now)
"$LAB_DIR/lab-with-vm" -- uname -a >"$tmp/uname.out" 2>"$tmp/uname.err"; rc=$?
t1=$(now)
if [ $rc = 0 ] && grep -q '^Darwin' "$tmp/uname.out"; then ok "uname in guest: $(cat "$tmp/uname.out")"; else bad "uname rc=$rc: $(tail -5 "$tmp/uname.err")"; fi
printf 'time: clone+boot+ssh+run+delete = %.1f s\n' "$(python3 -c "print($t1-$t0)")"
[ "$(run_vms)" = 0 ] && ok "no run VM left" || bad "run VM left behind"

echo "== 2. three concurrent jobs (2 slots)"
t0=$(now)
for n in 1 2 3; do
  ( "$LAB_DIR/lab-with-vm" --id "accept$n" -- sh -c 'python3 -c "import time;print(time.time())" 2>/dev/null || date +%s; sleep 20' >"$tmp/job$n.out" 2>"$tmp/job$n.err" ) &
  sleep 2
done
sleep 40
echo "VMs while 3 jobs are submitted: $(run_vms)  (slots: $(lab_slot status))"
[ "$(run_vms)" -le 2 ] && ok "never more than 2 VMs" || bad "more than 2 VMs"
wait
t1=$(now)
for n in 1 2 3; do echo "job$n started at: $(head -1 "$tmp/job$n.out") (rc file: $(wc -c <"$tmp/job$n.out") bytes)"; done
printf 'time: all three jobs = %.1f s\n' "$(python3 -c "print($t1-$t0)")"
python3 - "$tmp" <<'PY' && ok "third job started after one of the first two finished" || bad "third job did not queue"
import sys
t = []
for n in (1, 2, 3):
    try: t.append(float(open("%s/job%d.out" % (sys.argv[1], n)).readline()))
    except Exception: sys.exit(1)
t.sort()
sys.exit(0 if t[2] >= t[0] + 18 else 1)
PY

echo "== 3. cleanup"
[ "$(run_vms)" = 0 ] && ok "no ashdash-run-* VM left (tart list)" || bad "VMs left"
python3 "$LAB_DIR/slot.py" --run-dir "$LAB_RUN_DIR" status
python3 "$LAB_DIR/slot.py" --run-dir "$LAB_RUN_DIR" status | grep -qx 'slot0: free, slot1: free' && ok "both slots free" || bad "slots still held"
lab_vm_exists "$LAB_GOLDEN" && ok "golden image intact" || bad "golden image missing"

echo "== 4. disk"
"$TART_BIN" list --source local
du -sh "$TART_HOME" 2>/dev/null

[ "$fails" = 0 ] && echo "accept: OK" || { echo "accept: $fails FAILED"; exit 1; }

#!/usr/bin/env bash
# QA item 4 (last part): does the hotkey survive display sleep and system sleep? INSIDE a lab VM, DETACHED (the session may die):
#   nohup bash tests/lab/qa/sleep.sh >/dev/null 2>&1 &     then read $HOME/qa-sleep.txt (lab-run --no-sync -- cat ~/qa-sleep.txt)
# Writes one line per step; run it last in a VM, display sleep locks the guest session (docs/LAB.md) and may not come back.
# shellcheck disable=SC2015
set -uo pipefail
cd "$(dirname "$0")/../../.." || exit 1
export QA_ART=${QA_ART:-$HOME/qa-sleep-art} QA_RESULTS=${QA_RESULTS:-$HOME/qa-sleep-results.tsv}
# shellcheck source=checklist.sh
. tests/lab/qa/checklist.sh
OUT=$HOME/qa-sleep.txt; : >"$OUT"
say() { printf '%s %s\n' "$(date +%H:%M:%S)" "$*" >>"$OUT"; }
locked() { python3 - <<'P' 2>/dev/null || echo "?"
import Quartz
d = Quartz.CGSessionCopyCurrentDictionary() or {}
print("locked" if d.get("CGSSessionScreenIsLocked") else "unlocked")
P
}
reseed; start_app; hide_
show && say "before: hotkey shows the launcher" || say "before: hotkey did NOT show the launcher"; hide_
say "pid $(app_pid); session $(locked); display sleep supported: $(pmset -g cap 2>&1 | tr '\n' ' ')"
sudo -n pmset displaysleepnow; say "pmset displaysleepnow rc=$?"; sleep 12
say "after displaysleepnow: session $(locked); app alive=$(alive && echo yes || echo no)"
caffeinate -u -t 3 2>/dev/null; sleep 4
say "after caffeinate -u (user activity assertion): session $(locked)"
hotkey; sleep 1.5; visible && say "display sleep + wake: hotkey shows the launcher: yes (session $(locked))" || say "display sleep + wake: hotkey shows the launcher: NO (session $(locked))"; hide_
say "pmset sleepnow follows (this may end the ssh session and the VM)"
sudo -n pmset sleepnow; say "pmset sleepnow rc=$?"; sleep 20
say "after sleepnow: app alive=$(alive && echo yes || echo no); session $(locked)"
hotkey; sleep 1.5; visible && say "system sleep: hotkey shows the launcher: yes" || say "system sleep: hotkey shows the launcher: NO"; hide_
say "done"

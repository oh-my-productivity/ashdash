#!/usr/bin/env bash
# Desktop smoke test for the launcher UI glue (actions panel, workflow import prompt,
# Script Filter rerun/pending rows, Quick Look layering, held-modifier subtitle,
# buffer, "Open Launcher" outcome, scrolling Large Type).
#
# It SHOWS WINDOWS and drives System Events on the real desktop, so it holds the shared
# desktop lock (mkdir /tmp/ashdash-desktop.lock), runs with a throw-away HOME and a
# non-default hotkey, and always releases the lock. Screenshots (whole-desktop crops) go
# to $OUT; look at them, then DELETE them: they can contain the user's other windows.
#
#   scripts/qa/ui-glue-smoke.sh [path-to-ashdash-binary]
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${1:-$ROOT/target/debug/ashdash}"
LOCK=/tmp/ashdash-desktop.lock
T="$(mktemp -d /tmp/ashdash-uiglue.XXXXXX)"
OUT="$T/shots"
mkdir -p "$OUT" "$T/files/sub"
export HOME="$T/home"
mkdir -p "$HOME"
APP_PID=""
HAVE_LOCK=0

cleanup() {
    [ -n "$APP_PID" ] && kill "$APP_PID" 2>/dev/null
    [ "$HAVE_LOCK" = 1 ] && rmdir "$LOCK" 2>/dev/null
    echo "screenshots (delete after looking): $OUT"
}
trap cleanup EXIT

# --- the desktop lock: poll every 5 s for up to 10 minutes
for _ in $(seq 1 120); do
    if mkdir "$LOCK" 2>/dev/null; then HAVE_LOCK=1; break; fi
    sleep 5
done
[ "$HAVE_LOCK" = 1 ] || { echo "could not get $LOCK"; exit 2; }
START=$(date +%s)
if pgrep -x ashdash >/dev/null; then echo "an ashdash is already running"; exit 2; fi

# --- fixtures (all synthetic)
echo hello > "$T/files/report.txt"; echo hi > "$T/files/notes.txt"
python3 - "$T" <<'PY'
import plistlib, zipfile, sys, os
t = sys.argv[1]
pkg = {
 "name": "Smoke Sample", "bundleid": "qa.smoke.sample", "createdby": "Ann Author",
 "description": "Synthetic workflow for the UI smoke",
 "objects": [
  {"uid":"S1","type":"alfred.workflow.action.script","config":{"type":0,"script":"echo hi {query}","concurrently":False}},
  {"uid":"U1","type":"alfred.workflow.action.openurl","config":{"url":"https://example.com/{query}"}},
  {"uid":"K1","type":"alfred.workflow.input.keyword","config":{"keyword":"smk","withspace":True,"argumenttype":1,"text":"Smoke","subtext":""}},
  {"uid":"T1","type":"alfred.workflow.action.terminalcommand","config":{"text":"echo {query}"}},
 ],
 "connections":{"K1":[{"destinationuid":"S1","modifiers":0}]},
}
with zipfile.ZipFile(f"{t}/files/sample.alfredworkflow", "w") as z:
    z.writestr("info.plist", plistlib.dumps(pkg))
d = f"{t}/home/Library/Application Support/ashdash/workflows/user.workflow.smoke2"
os.makedirs(d, exist_ok=True)
live = {
 "name":"Smoke Live","bundleid":"qa.smoke.live",
 "objects":[
  {"uid":"SF","type":"alfred.workflow.input.scriptfilter","config":{"keyword":"sf","withspace":True,"argumenttype":1,"type":0,"scriptargtype":1,"title":"Lookup","subtext":"","runningsubtext":"Working on {query}…",
     "script":"sleep 0.9; s=$(date +%S); echo \"{\\\"rerun\\\":1,\\\"items\\\":[{\\\"title\\\":\\\"tick $s\\\",\\\"subtitle\\\":\\\"query $1\\\",\\\"valid\\\":false}]}\""}},
  {"uid":"LT","type":"alfred.workflow.input.scriptfilter","config":{"keyword":"lt","withspace":True,"argumenttype":1,"type":0,"scriptargtype":1,"title":"Long","subtext":"",
     "script":"text=$(yes 'lorem ipsum dolor sit amet' | head -n 900 | tr '\\n' ' '); echo \"{\\\"items\\\":[{\\\"title\\\":\\\"Long text\\\",\\\"valid\\\":false,\\\"text\\\":{\\\"largetype\\\":\\\"$text\\\"}}]}\""}},
  {"uid":"T","type":"alfred.workflow.trigger.external","config":{"triggerid":"t1"}},
  {"uid":"B","type":"alfred.workflow.action.browseinalfred","config":{"path": t + "/files/"}},
 ],
 "connections":{"T":[{"destinationuid":"B","modifiers":0}]},
}
open(f"{d}/info.plist","wb").write(plistlib.dumps(live))
PY

"$BIN" config set hotkey "ctrl+alt+shift+j" >/dev/null

# --- helpers
WIN="$T/windows"
swiftc -O "$ROOT/scripts/qa/windows.swift" -o "$WIN" 2>/dev/null || { echo "swiftc failed"; exit 2; }
# Capture only the launcher's own window (never the desktop around it).
shot() {
    sleep "${2:-0.9}"
    local id
    id=$("$WIN" ashdash | sort -k6,6nr | head -1 | cut -d' ' -f1)
    if [ -n "$id" ]; then
        screencapture -x -o -l "$id" "$OUT/$1.png" && sips -Z 800 "$OUT/$1.png" >/dev/null 2>&1
    else
        echo "no launcher window for $1"
    fi
}
# Which window is above which (front to back) and their levels, as text.
stack() { echo "  windows (id layer order x y w h owner):"; "$WIN" ashdash qlmanage | sed 's/^/    /'; }
SE='tell application "System Events"'
key()    { osascript -e "$SE to key code $1" >/dev/null; }
keymod() { osascript -e "$SE to key code $1 using {$2 down}" >/dev/null; }
typ()    { osascript -e "$SE to keystroke \"$1\"" >/dev/null; }
typmod() { osascript -e "$SE to keystroke \"$1\" using {$2 down}" >/dev/null; }
RIGHT=124 LEFT=123 DOWN=125 UP=126 RET=36 ESC=53 PGDN=121
step() { echo "[$(( $(date +%s) - START ))s] $*"; }

"$BIN" show >/dev/null 2>&1; sleep 3
APP_PID=$(pgrep -x ashdash | head -1)
step "launcher pid $APP_PID"

# 1+2: file result -> Right opens the actions panel; a workflow package gets Install first
"$BIN" search "$T/files/sam"; shot 01-results 1
key $RIGHT; shot 02-actions-for-package 0.6
# 3: Return on "Install Workflow..." -> preview + prompt; an early Return must not install
key $RET; sleep 0.2; key $RET; shot 03-prompt 1.2
"$BIN" workflows | grep -q "Smoke Sample" && step "FAIL: installed before confirmation" || step "ok: nothing installed yet"
key $ESC; shot 04-after-cancel 1.2
"$BIN" workflows | grep -q "Smoke Sample" && step "FAIL: installed after Esc" || step "ok: Esc installed nothing"
key $RET; sleep 1.5; key $RET; shot 05-installed 1.5
"$BIN" workflows | grep -q "Smoke Sample" && step "ok: installed after confirmation" || step "FAIL: not installed after confirmation"

# 4: Open With... one level deeper, filter, back
"$BIN" search "$T/files/rep"; sleep 0.8
key $RIGHT; sleep 0.5; key $DOWN; shot 06-actions-for-file 0.4
key $RIGHT; shot 07-open-with 2
typ "saf"; shot 08-open-with-filtered 0.6
key $ESC; sleep 0.3; key $ESC; shot 09-back-in-actions 0.5
key $LEFT; shot 10-back-in-results 0.6

# 5: Quick Look through the Os handle: the preview must be drawn above the launcher
typmod y command; shot 11-quicklook 2.5; stack
key $ESC; shot 12-after-quicklook 1.2

# 6: held modifier swaps the subtitle
keystroke_cmd_down() { osascript -e "$SE to key down command" >/dev/null; }
keystroke_cmd_up()   { osascript -e "$SE to key up command" >/dev/null; }
keystroke_cmd_down; shot 13-cmd-held 0.7; keystroke_cmd_up

# 7: Script Filter pending row + rerun
"$BIN" search "sf cats"; shot 14-pending 0.3
shot 15-tick-a 1.0
shot 16-tick-b 1.3

# 8: buffer
"$BIN" search "$T/files/"; sleep 0.8
keymod $UP option; key $DOWN; keymod $UP option; shot 17-buffer 0.5
key $RIGHT; shot 18-buffer-actions 0.6
key $ESC; sleep 0.3

# 9: Open Launcher outcome from an External Trigger
"$BIN" trigger qa.smoke.live t1; shot 19-show-launcher 1

# 10: Large Type that does not fit scrolls
"$BIN" search "lt x"; sleep 1.2
typmod l command; shot 20-largetype 1.2
key $PGDN; key $PGDN; shot 21-largetype-scrolled 0.8
key $ESC; sleep 0.3
"$BIN" quit >/dev/null 2>&1
step "done"

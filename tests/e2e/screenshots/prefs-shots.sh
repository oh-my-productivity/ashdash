#!/usr/bin/env bash
# Screenshots of the real preferences window for every page, in dark and light, taken on a GitHub-hosted macOS runner (CI only: it drives
# the real keyboard, screen and windows, so it never runs on a developer's Mac; docs/TESTING.md):
#
#   bash tests/e2e/ci/run-screenshots.sh          (the `screenshots` job of e2e.yml uploads $E2E_ARTIFACTS/shots as an artifact)
#
# What it does (cwd = the checkout):
#   1. builds the debug binary and grants TCC in this throw-away runner (tests/e2e/guest/tcc-grant.sh; E2E_AUTO_TCC=0 skips),
#   2. seeds a temporary HOME / data folder: one workflow, one snippet collection, two web searches (one with an invalid
#      URL), Safari bookmarks on (the default), a "launcher" status file whose pid is a live `sleep`. The runner does not
#      enforce Full Disk Access on Terminal (a listing of ~/Library/Safari works), so the refusal is simulated with
#      ASHDASH_PREFS_FAKE_PERMISSIONS=fda=denied (see mac/permissions.rs); the real probes are printed to probes.txt before
#      and after the TCC grant,
#   3. for dark and light (ASHDASH_PREFS_THEME) and for 760x560 and 640x480: opens `ashdash prefs-window` in the GUI session
#      (tests/e2e/guest/gui-exec), walks Cmd+1 ... Cmd+7 with System Events, and takes a window-only screenshot per page
#      (screencapture -l <window id> -o) plus the recorder states, the Tab focus ring, "/" (focus the search field), Esc in it and Cmd+R.
# Output: $E2E_ARTIFACTS/shots/<theme>-<size>-<name>.png, probes.txt, window.txt. The screenshots are workflow artifacts (7 days), never
# committed; the reference images in docs/design/settings-v4/ are curated by hand.
# shellcheck disable=SC2015
set -uo pipefail
cd "$(dirname "$0")/../../.." || exit 1
ROOT=$PWD
ART=${E2E_ARTIFACTS:-$(mktemp -d "${TMPDIR:-/tmp}/prefs-shots.XXXXXX")}
SHOTS=$ART/shots
mkdir -p "$SHOTS"
export PATH="$HOME/.cargo/bin:$PATH"
GUI_EXEC=$ROOT/tests/e2e/guest/gui-exec
SIZES=${SHOT_SIZES:-"760x560 640x480"}
THEMES=${SHOT_THEMES:-"dark light"}
log() { echo "[shots] $*" | tee -a "$ART/log.txt"; }

cargo build -p ashdash >"$ART/build.log" 2>&1 || { tail -20 "$ART/build.log"; exit 1; }
BIN=$ROOT/target/debug/ashdash
{ echo "== real probes from the ssh session, before the TCC grant"; cargo test -p ashdash --bin ashdash real_probes -- --ignored --nocapture 2>&1 | grep PROBES; } >"$ART/probes.txt"

if [ "${E2E_AUTO_TCC:-1}" = 1 ] && csrutil status 2>&1 | grep -q disabled; then
  bash "$ROOT/tests/e2e/guest/tcc-grant.sh" >"$ART/tcc-grant.txt" 2>&1 && log "tcc-grant ok" || log "tcc-grant FAILED (see tcc-grant.txt)"
fi
{ echo "== real probes after the TCC grant"; cargo test -p ashdash --bin ashdash real_probes -- --ignored --nocapture 2>&1 | grep PROBES; } >>"$ART/probes.txt"
PERMS=${SHOT_PERMS:-fda=denied}

# ---- seed ------------------------------------------------------------------------------------------------------------
WORK=$(mktemp -d "${TMPDIR:-/tmp}/prefs-shots-work.XXXXXX")
export SEED_HOME=$WORK/home
DATA="$SEED_HOME/Library/Application Support/ashdash"
mkdir -p "$DATA/workflows/user.workflow.demo" "$DATA/run"
sleep 3600 &
SLEEP_PID=$!
cleanup() {
  kill "$SLEEP_PID" 2>/dev/null
  pkill -f "$BIN prefs-window" 2>/dev/null
  rm -rf "$WORK"
}
trap cleanup EXIT
cat >"$DATA/config.json" <<'JSON'
{"web_searches":[
 {"keyword":"g","title":"Google","url":"https://www.google.com/search?q={query}","fallback":true},
 {"keyword":"bad","title":"Broken search","url":"file:///etc/{query}","fallback":false}]}
JSON
cat >"$DATA/workflows/user.workflow.demo/info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
<key>name</key><string>Demo workflow</string><key>bundleid</key><string>demo.workflow</string><key>version</key><string>1.2</string>
<key>objects</key><array/><key>connections</key><dict/></dict></plist>
PLIST
echo '[{"name":"Signature","keyword":"sig","text":"Best regards","collection":"Work"},{"name":"Address","keyword":"addr","text":"1 Main St","collection":"Work"}]' >"$DATA/snippets.json"
write_status() { # problems as a JSON array
  printf '{"pid":%s,"updated":%s,"problems":%s,"restart_required":[],"hotkey_active":"alt+shift+Space"}' "$SLEEP_PID" "$(date +%s)" "${1:-[]}" >"$DATA/run/reload-status.json"
}
write_status '[]'

# ---- helpers ----------------------------------------------------------------------------------------------------------
se() { perl -e 'alarm shift; exec @ARGV' 25 osascript -e "tell application \"System Events\" to $1" 2>&1; }
frontmost() { se 'set frontmost of process "ashdash" to true' >/dev/null; sleep 0.5; }
key() { # key <code> <modifiers or ""> <seconds to wait>
  local using=""
  [ -n "${2:-}" ] && using=" using {$2}"
  se "key code $1$using" >/dev/null; sleep "${3:-0.6}"
}
cmd() { key "$1" "command down" "${2:-0.8}"; }
win_id() {
  osascript -l JavaScript -e '
    ObjC.import("CoreGraphics");
    var l = ObjC.deepUnwrap(ObjC.castRefToObject($.CGWindowListCopyWindowInfo(1, 0)));
    l.filter(function(w){ return w.kCGWindowOwnerName == "ashdash" && w.kCGWindowLayer == 0; })
     .map(function(w){ return w.kCGWindowNumber; }).join(" ")' 2>/dev/null | awk '{print $1}'
}
shot() { # shot <name>
  local id; id=$(win_id)
  [ -n "$id" ] || { log "no ashdash window for $1"; return 1; }
  screencapture -l "$id" -o "$SHOTS/$THEME-$SIZE-$1.png" 2>>"$ART/log.txt" && log "shot $THEME-$SIZE-$1 ($(stat -f %z "$SHOTS/$THEME-$SIZE-$1.png") bytes)"
}
launch() {
  pkill -f "$BIN prefs-window" 2>/dev/null; sleep 1
  HOME=$SEED_HOME ASHDASH_PREFS_THEME=$THEME ASHDASH_PREFS_FAKE_PERMISSIONS=$PERMS "$GUI_EXEC" --detach -- "$BIN" prefs-window >>"$ART/log.txt" 2>&1
  for _ in $(seq 1 40); do [ -n "$(win_id)" ] && break; sleep 1; done
  sleep 2
  frontmost
  local w=${SIZE%x*} h=${SIZE#*x}
  se 'tell process "ashdash" to set position of window 1 to {80, 60}' >/dev/null
  se "tell process \"ashdash\" to set size of window 1 to {$w, $h}" >/dev/null
  sleep 1.5
  se 'tell process "ashdash" to get {position, size} of window 1' >>"$ART/window.txt"
}
# A real mouse click: move first (winit only knows the pointer position from move events, so System Events' bare
# `click at` lands nowhere), then press and release. Points, screen coordinates.
mclick() {
  osascript -l JavaScript -e '
    ObjC.import("CoreGraphics");
    function ev(t, x, y) { $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent($(), t, $.CGPointMake(x, y), 0)); }
    ev($.kCGEventMouseMoved, '"$1"', '"$2"'); delay(0.3);
    ev($.kCGEventLeftMouseDown, '"$1"', '"$2"'); delay(0.08);
    ev($.kCGEventLeftMouseUp, '"$1"', '"$2"');' >/dev/null 2>&1
  sleep 0.5
}
# A click on empty sidebar space takes keyboard focus off any field (Return then reaches the page). Not on the title bar:
# macOS treats that as a window drag and the plain keys that follow never arrive.
refocus() { local h=${SIZE#*x}; mclick 160 $((60 + h - 90)); }
# key codes: 18..23,26,28 = digits 1..8 on a US layout; 15 = r, 53 = esc, 36 = return, 48 = tab, 51 = delete, 49 = space, 0 = a
DIGIT=(0 18 19 20 21 23 22 26)

for THEME in $THEMES; do
  for SIZE in $SIZES; do
    log "== $THEME $SIZE"
    launch
    for page in "1 general" "2 search" "3 clipboard" "4 snippets" "5 workflows" "6 permissions" "7 about"; do
      n=${page%% *}; name=${page#* }
      cmd "${DIGIT[$n]}" 1.2
      shot "$name"
    done
    cmd "${DIGIT[1]}" 1
    # Interactions that need plain keys run in a freshly launched window with no screenshot before them: after a
    # screencapture the window is no longer the key window and Esc / Tab / Return / letters are not delivered.
    w=${SIZE%x*}
    rx=$((80 + 180 + 188 + (w - 160 - 40 - 188) / 2)); ry=$((60 + 130))
    chain() { local name=$1; shift; launch; "$@"; shot "$name"; }
    # "/" focuses "Go to a setting..." when no text field has focus (typed as a character, so any keyboard layout)
    a_slash() { se 'keystroke "/"' >/dev/null; sleep 0.6; }
    a_slash_work() { a_slash; se 'keystroke "work"' >/dev/null; sleep 0.6; }
    a_slash_esc() { a_slash_work; key 53 "" 0.8; }
    a_tab() { mclick 160 $((60 + ${SIZE#*x} - 90)); for _ in 1 2 3 4 5 6 7 8; do key 48 "" 0.35; done; }
    a_return() { mclick 160 $((60 + ${SIZE#*x} - 90)); key 36 "" 1.2; }
    a_rec() { mclick "$rx" "$ry"; sleep 0.5; }
    a_rec_refuse() { a_rec; key 0 "" 0.8; }
    a_rec_esc() { a_rec_refuse; key 53 "" 0.8; }
    a_rec_backspace() { a_rec; key 51 "" 1.0; }
    [ "$SIZE" = 760x560 ] && chain slash-focus a_slash
    [ "$SIZE" = 760x560 ] && chain slash-work a_slash_work
    [ "$SIZE" = 760x560 ] && chain slash-esc a_slash_esc
    [ "$SIZE" = 760x560 ] && chain tab-focus a_tab
    [ "$SIZE" = 760x560 ] && chain after-return-fix a_return
    [ "$SIZE" = 760x560 ] && chain recorder-recording a_rec
    [ "$SIZE" = 760x560 ] && chain recorder-refused a_rec_refuse
    [ "$SIZE" = 760x560 ] && chain recorder-idle-after-esc a_rec_esc
    [ "$SIZE" = 760x560 ] && chain recorder-after-backspace a_rec_backspace
    launch
    # keyboard start: Tab to the box is long; Cmd+R re-check on Permissions
    cmd "${DIGIT[6]}" 1; cmd 15 1.2; shot "permissions-recheck"
    # a launcher problem (what a skipped web search looks like to this window)
    write_status '["web search bad skipped: only http and https URLs are allowed"]'; sleep 2
    cmd "${DIGIT[1]}" 1.2; shot "general-problem"
    write_status '[]'; sleep 1.5
  done
done
if [ "$SIZES" != "" ]; then
  # mixed permission states on the Permissions page
  SIZE=760x560; PERMS="accessibility=denied,automation=ask,fda=denied"
  for THEME in $THEMES; do launch; cmd "${DIGIT[6]}" 1.5; shot "permissions-mixed"; done
fi
log "done"

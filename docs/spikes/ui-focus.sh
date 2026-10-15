#!/usr/bin/env bash
# Focus-return spike: repeatedly show the launcher over different frontmost apps,
# type into it, hide it, and check that (a) it took key focus, (b) the typed text
# reached the query field, (c) the previous app is frontmost again.
#
# usage: [HIDE=esc|hotkey] [DELAY=0.8] docs/spikes/ui-focus.sh [binary] [iterations]
#   HIDE=esc     hide with Esc Esc (default); HIDE=hotkey hide by pressing the hotkey again
#   DELAY=0.05   seconds between the hotkey and the first typed key (typing-race test)
# Needs Accessibility permission for the terminal (osascript key events).
# Uses a throw-away HOME and a non-default hotkey (Ctrl+Opt+Shift+J) so it can
# run next to a real launcher. Do not touch the keyboard while it runs.
set -u
BIN=${1:-target/debug/ashdash}
N=${2:-10}
HIDE=${HIDE:-esc}
DELAY=${DELAY:-0.8}
APPS=(Finder Calculator Terminal Safari "System Settings")
TMP=$(mktemp -d)
LOG=$TMP/trace.log
CP=/tmp/wp7-ashdash-spike   # unique name so pkill cannot hit other launchers
cp "$BIN" "$CP"

export HOME=$TMP
mkdir -p "$HOME/Library/Application Support/ashdash"
echo '{"version":1,"hotkey":"ctrl+alt+shift+KeyJ"}' > "$HOME/Library/Application Support/ashdash/config.json"
ASHDASH_TRACE=1 "$CP" > "$LOG" 2>&1 &
sleep 5

# NSWorkspace sees accessory (no-Dock) apps; System Events' "frontmost" does not.
front() { osascript -l JavaScript -e 'ObjC.import("AppKit"); $.NSWorkspace.sharedWorkspace.frontmostApplication.localizedName.js'; }
hotkey() { osascript -e 'tell application "System Events" to key code 38 using {control down, option down, shift down}'; }

pass=0
for i in $(seq 1 "$N"); do
  app=${APPS[$(( (i - 1) % ${#APPS[@]} ))]}
  osascript -e "tell application \"$app\" to activate" >/dev/null 2>&1
  sleep 0.9
  before=$(front)
  mark=$(wc -l < "$LOG")
  hotkey
  sleep "$DELAY"
  osascript -e "tell application \"System Events\" to keystroke \"zq$i\""
  sleep 0.5
  during=$(front)
  if [ "$HIDE" = hotkey ]; then
    hotkey
  else
    # First Esc clears the query, the second hides the window.
    osascript -e 'tell application "System Events" to key code 53'
    sleep 0.3
    osascript -e 'tell application "System Events" to key code 53'
  fi
  sleep 0.8
  after=$(front)
  new=$(tail -n +"$((mark + 1))" "$LOG")
  focus=$(grep -c 'focus=Some(true)' <<<"$new")
  typed=$(grep -c "query=\"zq$i\"" <<<"$new")
  ok=FAIL
  if [ "$focus" -ge 1 ] && [ "$typed" -ge 1 ] && [ "$after" = "$before" ]; then ok=ok; pass=$((pass + 1)); fi
  echo "run $i app=$app before=[$before] during=[$during] focus_events=$focus typed=$typed after=[$after] => $ok"
done
echo "passed $pass/$N"
pkill -f "$CP"
rm -f "$CP"
echo "trace log: $LOG"

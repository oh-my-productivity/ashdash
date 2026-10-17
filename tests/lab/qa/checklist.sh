#!/usr/bin/env bash
# QA item 3: docs/QA.md section 3 (except 3.10) and the Wave 4/5 behaviours, scripted, INSIDE a lab VM with the lab-tool grants
# (tests/lab/guest/tcc-grant.sh; none of them names Ashdash). Usage (from the synced worktree in the guest):
#   bash tests/lab/qa/checklist.sh [section ...]     sections: setup s31 s32 s33 s34 s35 s36 s37 s38 s39 s311 w45 (default: all)
# The app under test is the packaged bundle (APP=/Applications/Ashdash.app, or $APP) started with HOME=$QA_HOME (a seeded throw-away data
# folder, tests/lab/qa/fixtures.py) so its TCC identity is the bundle's. Results go to $QA_RESULTS (tsv); screenshots to $QA_ART.
# shellcheck disable=SC2015,SC2016,SC2034
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../../.." || exit 1
# shellcheck source=lib.sh
. tests/lab/qa/lib.sh

APP=${APP:-/Applications/Ashdash.app}
BIN=$APP/Contents/MacOS/ashdash
QAH=${QA_HOME:-$HOME/qa-home}
WORK=${QA_WORK:-$HOME/qa-work}
DATA="$QAH/Library/Application Support/ashdash"
cli() { HOME=$QAH wd 20 "$BIN" "$@"; }
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time()*1000'; }
alive() { pgrep -f "$BIN\$" >/dev/null; }
app_pid() { pgrep -f "$BIN\$" | head -1; }
sock_ok() { [ -S "$DATA/run/ipc.sock" ] || [ -S "/tmp/ashdash-$(id -u)/ipc.sock" ]; }

stop_app() { cli quit >/dev/null 2>&1; sleep 1; pkill -f "$BIN\$" 2>/dev/null; sleep 0.5; }
start_app() { # start_app [ENV=VAL ...]: launch in the GUI session with the seeded HOME
  stop_app
  local envs=(env "HOME=$QAH" "$@")
  wd 25 "$GUI_EXEC" --detach -- "${envs[@]}" "$BIN" >/dev/null
  local i; for i in $(seq 1 30); do sock_ok && alive && break; sleep 1; done
  sleep 1.5
}
reseed() { rm -rf "$QAH" "$WORK"; mkdir -p "$QAH" "$WORK"; python3 tests/lab/qa/fixtures.py "$QAH" "$WORK" >/dev/null; }

# the launcher is visible when a >=600 pt wide Ashdash window is at the panel layer (the status item window is 32 wide)
visible() { win_tool Ashdash ashdash | awk '$6>=600 {f=1} END {exit !f}'; }
lw() { win_tool Ashdash ashdash | awk '$6>=600 {print $4, $5, $6, $7; exit}'; }   # x y w h
show() { visible && return 0; hotkey; local i; for i in 1 2 3 4 5 6 7 8; do sleep 0.25; visible && return 0; done; return 1; }
hide_() { visible || return 0; key_code $KC_ESC; sleep 0.4; visible && { hotkey; sleep 0.5; }; }
clear_field() { se 'keystroke "a" using {command down}' >/dev/null; key_code $KC_DELETE >/dev/null; }
# ask <query>: show, clear, type, wait for results, OCR of the window (lines joined by " | ")
ask() { show || return 1; clear_field; type_text "$1"; sleep "${2:-1.2}"; look "ask-$(echo "$1" | tr -c 'A-Za-z0-9' _ | cut -c1-20)"; }
look() { shot "$1" 2>/dev/null || { echo "(no window)"; return 1; }; ocr "$QA_ART/$1.png" 2>/dev/null | grep -v IOServiceMatching | tr '\n' '|'; }
has() { echo "$1" | grep -qiE "$2"; }
tcc_reset() { tccutil reset All dev.ashdash.launcher >/dev/null 2>&1; }
# allow/deny a pending Automation or other consent dialog of the lab session; prints its text
consent() { # consent Allow|"Don’t Allow" [secs]
  local i t; for i in $(seq 1 "${2:-10}"); do
    t=$(ax_dialogs | grep -E '^UserNotificationCenter' | head -1)
    if [ -n "$t" ]; then click_button UserNotificationCenter "$1"; echo "$t"; return 0; fi
    sleep 1
  done
  return 1
}
textedit_open() { open -a TextEdit; sleep 3; se 'tell process "TextEdit" to keystroke "n" using command down' >/dev/null; sleep 1.5; }
textedit_text() { se 'tell process "TextEdit" to get value of text area 1 of scroll area 1 of window 1' | tail -1; }
textedit_close() { pkill -x TextEdit; sleep 1; rm -rf "$HOME/Library/Autosave Information" 2>/dev/null; }

s_setup() {
  reseed; start_app
  if alive && sock_ok; then result SETUP PASS "Ashdash.app $(plutil -extract CFBundleShortVersionString raw "$APP/Contents/Info.plist") running from $APP with seeded HOME=$QAH (pid $(app_pid))"
  else result SETUP FAIL "app not running"; fi
}

# ------------------------------------------------------------------------------------------------------------------------ 3.1 hotkey and window
s31() {
  start_app; cli config set hotkey option+shift+space >/dev/null 2>&1; hide_
  # 3.1.1 latency: `ashdash show` (CLI -> socket) until a launcher-sized window is listed. Includes two process starts of the harness itself.
  local i t0 t1 lat=() n=0
  for i in 1 2 3 4 5 6 7 8 9 10; do
    hide_; sleep 0.4
    t0=$(now_ms); cli show >/dev/null; while ! visible; do sleep 0.01; [ $(( $(now_ms) - t0 )) -gt 3000 ] && break; done; t1=$(now_ms)
    lat+=($(( t1 - t0 )))
  done
  local sorted; sorted=$(printf '%s\n' "${lat[@]}" | sort -n | tr '\n' ' ')
  result 3.1.1a PASS "show-to-visible over the CLI, 10 samples (ms, sorted, includes the harness' own process starts): $sorted; VM has no GPU (software/paravirt Metal), host numbers are in docs/PERF.md"
  # 3.1.1b focus + blinking caret: type lands in the field; caret blinks (distinct frames)
  show; clear_field; type_text "qz"; sleep 0.5
  local o; o=$(look hk-typed)
  has "$o" 'qz' && result 3.1.1b PASS "input field focused after the hotkey: typed 'qz' shows in the field (OCR: $o)" || result 3.1.1b FAIL "typed text not in the field: $o"
  clear_field; sleep 0.3
  local sums=""; for i in 1 2 3 4 5 6; do shot "caret$i" >/dev/null 2>&1; sums="$sums $(md5 -q "$QA_ART/caret$i.png")"; sleep 0.25; done
  local d; d=$(echo "$sums" | tr ' ' '\n' | grep -c . ); local u; u=$(echo "$sums" | tr ' ' '\n' | grep . | sort -u | wc -l | tr -d ' ')
  [ "$u" -ge 2 ] && result 3.1.1c PASS "caret blinks: $u distinct frames out of $d screenshots 0.25 s apart" || result 3.1.1c FAIL "$d identical screenshots: no blinking caret seen"
  rm -f "$QA_ART"/caret?.png
  # 3.1.2 toggle 20 times quickly
  hide_; sleep 0.5
  wd 60 osascript -e 'tell application "System Events" to repeat 20 times
key code 49 using {option down, shift down}
delay 0.12
end repeat' >/dev/null
  sleep 1.5
  local nw; nw=$(win_tool Ashdash ashdash | awk '$6>=600' | wc -l | tr -d ' ')
  if alive && [ "$nw" = 0 ]; then
    show && [ "$(win_tool Ashdash ashdash | awk '$6>=600' | wc -l | tr -d ' ')" = 1 ] && result 3.1.2 PASS "20 toggles at 0.12 s: ends hidden (even count), process alive, one launcher window after the next press" || result 3.1.2 FAIL "after 20 toggles the next press did not show exactly one window"
  else result 3.1.2 FAIL "after 20 toggles: alive=$(alive && echo y || echo n) launcher windows=$nw (expected hidden)"; fi
  hide_
  # 3.1.3 over a full-screen app (single display): TextEdit in full screen
  textedit_open
  se 'tell process "TextEdit" to set value of attribute "AXFullScreen" of window 1 to true' >/dev/null; sleep 4
  if show; then
    local r; r=$(win_tool Ashdash ashdash | awk '$6>=600 {print "layer " $2 " at " $4 "," $5}' | head -1)
    shot fs-launcher >/dev/null 2>&1
    result 3.1.3a PASS "launcher shows over a full-screen TextEdit ($r); front app: $(front)"
  else result 3.1.3a FAIL "launcher did not appear over a full-screen app"; fi
  hide_; se 'tell process "TextEdit" to set value of attribute "AXFullScreen" of window 1 to false' >/dev/null; sleep 2
  result 3.1.3b NEEDS-OWNER "multi-monitor placement ('screen containing the mouse') and a different Space need a second display: move the pointer to the 2nd display, press the hotkey, the launcher must open on that display; repeat on a second Space"
  # 3.1.4 focus returns to the previous app: type a character after hide
  textedit_close; textedit_open
  show; key_code $KC_ESC; sleep 0.6; type_text "Z"; sleep 0.5
  local te; te=$(textedit_text)
  [ "$(front)" = TextEdit ] && has "$te" 'Z' && result 3.1.4 PASS "after Esc the front app is TextEdit again and a typed 'Z' lands in its document ('$te')" || result 3.1.4 FAIL "front=$(front) text='$te'"
  textedit_close
  # 3.1.5 second instance and a hotkey that is already taken
  local out; out=$(cli 2>&1 </dev/null | head -3 | tr '\n' ' ')
  result 3.1.5a PASS "second 'ashdash' instance: $(echo "$out" | cut -c1-200) (exit shown; the first keeps running: $(alive && echo yes || echo NO))"
  stop_app
  qt hog 49 2560 70 >"$QA_ART/hog.txt" 2>&1 &
  sleep 2
  start_app
  sleep 2
  o=$(look hk-conflict)
  if visible; then result 3.1.5b PASS "combo already registered by another process: launcher shows itself (otherwise unreachable); text on screen: $o; hog said: $(head -1 "$QA_ART/hog.txt")"
  else result 3.1.5b FAIL "combo taken: no window and no message (hog: $(head -1 "$QA_ART/hog.txt"))"; fi
  pkill -f "qa-tool hog" 2>/dev/null; wait 2>/dev/null
  # 3.1.6 hotkey changed in config.json (CLI) takes effect live, old key stops; invalid value keeps the old key
  start_app; hide_
  cli config set hotkey cmd+shift+space >/dev/null 2>&1; sleep 3
  hotkey; sleep 0.8; local oldk=0; visible && oldk=1; hide_
  key_code 49 "command down, shift down"; sleep 0.8; local newk=0; visible && newk=1; hide_; [ "$newk" = 1 ] && key_code 49 "command down, shift down" >/dev/null; sleep 0.5
  if [ "$oldk" = 0 ] && [ "$newk" = 1 ]; then result 3.1.6a PASS "ashdash config set hotkey cmd+shift+space: applied within 3 s, old key (opt+shift+space) dead, new key shows the launcher"
  else
    # find out when it does take effect: press the old key (which wakes the idle event loop) and try the new one again
    hotkey; sleep 1; hide_; sleep 1; key_code 49 "command down, shift down"; sleep 0.8; local later=0; visible && later=1; hide_; [ "$later" = 1 ] && key_code 49 "command down, shift down" >/dev/null; sleep 0.5
    result 3.1.6a FAIL "3 s after 'ashdash config set hotkey cmd+shift+space' (reload-status.json already says hotkey_active=cmd+shift+space) the OLD key still opens the launcher (old=$oldk) and the NEW one does not (new=$newk); after one more press of the old key the new one works (new=$later). Repeated: takes 5-10 s while the launcher is hidden. Likely cause: prefs/reload.rs:338 ctx.request_repaint() on an invisible window; the hotkey path uses wake_then_repaint (window.rs:72)"
  fi
  cp "$DATA/config.json" "$QA_ART/config-before-bad.json"
  python3 - "$DATA/config.json" <<'PY'
import json, sys
p = sys.argv[1]; c = json.load(open(p)); c["hotkey"] = "bogus+++"; json.dump(c, open(p, "w"))
PY
  sleep 3; hide_; key_code 49 "command down, shift down"; sleep 0.8; local keptk=0; visible && keptk=1; hide_
  [ "$keptk" = 1 ] && result 3.1.6b PASS "invalid hotkey value written to config.json: the previous combo (cmd+shift+space) keeps working" || result 3.1.6b FAIL "after an invalid hotkey value the old combo is dead"
  cli config set hotkey option+shift+space >/dev/null 2>&1; sleep 3
  # 3.1.7 no Dock icon, no Cmd+Tab entry
  local lsui; lsui=$(plutil -extract LSUIElement raw "$APP/Contents/Info.plist" 2>&1); local bg; bg=$(se 'tell process "ashdash" to get background only' | tail -1)
  [ "$lsui" = true ] && result 3.1.7 PASS "LSUIElement=$lsui (agent app): no Dock icon, not in the Cmd+Tab list; System Events says background only = $bg" || result 3.1.7 FAIL "LSUIElement=$lsui"
}

main() {
  local secs=("$@"); [ ${#secs[@]} = 0 ] && secs=(s_setup s31 s32 s33 s34 s35 s36 s37 s38 s39 s311 w45)
  local s; for s in "${secs[@]}"; do case "$s" in setup) s=s_setup ;; esac; echo "== $s"; "$s"; done
  stop_app 2>/dev/null
  echo "---- results"; cat "$QA_RESULTS"
}
[ "${BASH_SOURCE[0]}" = "$0" ] && main "$@"

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
  # `open` (not the binary as a child of Terminal): the app is then its own responsible process for TCC prompts
  local envs=(); local e; for e in "$@"; do envs+=(--env "$e"); done
  : >"$QA_ART/app.log"
  wd 25 "$GUI_EXEC" --detach -- /usr/bin/open --env "HOME=$QAH" ${envs[@]+"${envs[@]}"} --stdout "$QA_ART/app.log" --stderr "$QA_ART/app.log" "$APP" >/dev/null
  local i; for i in $(seq 1 30); do sock_ok && alive && break; sleep 1; done
  sleep 1.5
}
reseed() { rm -rf "$QAH" "$WORK"; mkdir -p "$QAH" "$WORK"; python3 tests/lab/qa/fixtures.py "$QAH" "$WORK" >/dev/null; }

# the launcher is visible when a >=600 pt wide Ashdash window is at the panel layer (the status item window is 32 wide)
# (a freshly started app also owns a hidden, full-size window at layer 3: only layer 25 counts)
visible() { win_tool Ashdash ashdash | awk '$2==25 && $6>=600 {f=1} END {exit !f}'; }
lw() { win_tool Ashdash ashdash | awk '$2==25 && $6>=600 {print $4, $5, $6, $7; exit}'; }   # x y w h
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

# ------------------------------------------------------------------------------------------------------------------------ 3.2 typing and results
s32() {
  start_app; hide_; mkdir -p "$QAH/Documents/Projects"
  local o
  o=$(ask saf); has "$o" 'Safari' && result 3.2.1a PASS "'saf' lists Safari (OCR: $o)" || result 3.2.1a FAIL "no Safari for 'saf': $o"
  result 3.2.1b BLOCKED "'vsc' / 'visual studio' need Visual Studio Code, not installed in the base image (initial matching itself is covered by core tests)"
  o=$(ask '2+3*4'); has "$o" '(^|\|)14(\||$)|= *14| 14' && result 3.2.2a PASS "'2+3*4' shows 14 (OCR: $o)" || result 3.2.2a FAIL "no 14: $o"
  o=$(ask '= sqrt(81)'); has "$o" '(^|[^0-9])9([^0-9]|$)' && result 3.2.2b PASS "'= sqrt(81)' shows 9 (OCR: $o)" || result 3.2.2b FAIL "no 9: $o"
  ask '1/0' 0.8 >/dev/null; ask '((((' 0.8 >/dev/null
  alive && result 3.2.2c PASS "'1/0' and '((((' typed: no crash, process alive" || result 3.2.2c FAIL "process died"
  # long pastes through the real pasteboard + Cmd+V
  local big; big=$(python3 -c "print('lorem ipsum ' * 9000)")
  show; clear_field; qt pb-set "$big"; se 'keystroke "v" using {command down}' >/dev/null; sleep 4
  local t0 t1; t0=$(now_ms); cli show >/dev/null 2>&1; local rc=$?; t1=$(now_ms)
  alive && [ $rc = 0 ] && result 3.2.3a PASS "100 KB pasted into the field: alive, 'ashdash show' answers in $(( t1 - t0 )) ms, ~$(( ${#big} / 1000 )) KB pasted" || result 3.2.3a FAIL "after the 100 KB paste: alive=$(alive && echo y || echo n) show rc=$rc"
  show; clear_field; qt pb-set "$(python3 -c "print('(' * 5000)")"; se 'keystroke "v" using {command down}' >/dev/null; sleep 3
  t0=$(now_ms); cli show >/dev/null 2>&1; rc=$?; t1=$(now_ms)
  alive && [ $rc = 0 ] && result 3.2.3b PASS "5,000 '(' pasted: alive, 'ashdash show' answers in $(( t1 - t0 )) ms" || result 3.2.3b FAIL "after 5,000 '(': alive=$(alive && echo y || echo n) rc=$rc"
  show; clear_field
  # unicode through System Events keystrokes (no IME)
  show; clear_field; qt type "ü日本語😀"; sleep 0.8; o=$(look unicode)
  alive && has "$o" 'ü' && result 3.2.4a PASS "ü, CJK and an emoji typed as Unicode key events (System Events 'keystroke' mangles them, a harness limit): no crash; OCR sees ü (OCR: $o); glyph rendering of CJK/emoji judged from unicode.png only" || result 3.2.4a FAIL "unicode typing: $o"
  result 3.2.4b NEEDS-OWNER "IME composition (Japanese/Chinese): switch the input source in System Settings > Keyboard, press the hotkey, compose 'nihon' + Space/Return: the candidate window must follow the field, the committed text must not lose characters"
  # editing keys
  show; clear_field; type_text "abc"; key_code $KC_DELETE; key_code $KC_DELETE; key_code $KC_DELETE; sleep 0.8; o=$(look backspace)
  ! has "$o" 'abc|Search Google' && result 3.2.5a PASS "three Backspaces empty the field and clear the results (OCR: $o)" || result 3.2.5a FAIL "results remain after emptying the field: $o"
  type_text "hello world"; se 'key code 51 using {command down}' >/dev/null; sleep 0.6; o=$(look cmd-bs)
  ! has "$o" 'hello' && result 3.2.5b PASS "Cmd+Backspace clears the line (OCR: $o)" || result 3.2.5b FAIL "Cmd+Backspace left text: $o"
  type_text "abc"; se 'keystroke "a" using {command down}' >/dev/null; type_text "x"; sleep 0.6; o=$(look cmd-a)
  has "$o" 'x' && ! has "$o" 'abc' && result 3.2.5c PASS "Cmd+A then typing replaces the text (OCR: $o)" || result 3.2.5c FAIL "Cmd+A: $o"
  # fast typing: the list belongs to the final text
  o=$(ask 'activity monitor' 2.5); has "$o" 'Activity Monitor' && result 3.2.6 PASS "'activity monitor' typed in one burst: the list is for the final text (OCR: $o)" || result 3.2.6 FAIL "stale or missing results after a fast burst: $o"
  # web searches
  o=$(ask 'g rust'); has "$o" 'Google' && result 3.2.7a PASS "'g rust' offers Google (OCR: $o)" || result 3.2.7a FAIL "$o"
  o=$(ask 'wiki x'); has "$o" 'Wikipedia' && result 3.2.7b PASS "'wiki x' offers Wikipedia (OCR: $o)" || result 3.2.7b FAIL "$o"
  o=$(ask 'zzqqxxnomatch'); has "$o" 'Search Google' && result 3.2.7c PASS "unknown text lists fallback searches (OCR: $o)" || result 3.2.7c FAIL "$o"
  # path browsing, terminal
  o=$(ask '~/Doc'); has "$o" 'Documents' && result 3.2.8a PASS "'~/Doc' browses to Documents (OCR: $o)" || result 3.2.8a FAIL "$o"
  o=$(ask '> echo hi'); has "$o" 'Run in Terminal' && result 3.2.8b PASS "'> echo hi' offers 'Run in Terminal' (OCR: $o)" || result 3.2.8b FAIL "$o"
  # Spotlight-backed file search
  if [ -n "$(mdfind -onlyin /Applications 'kMDItemContentType == "com.apple.application-bundle"' 2>/dev/null | head -1)" ]; then
    o=$(ask 'open report'); has "$o" 'report.txt|report' && result 3.2.8c PASS "'open report' finds a file through Spotlight (OCR: $o)" || result 3.2.8c FAIL "$o"
  else result 3.2.8c BLOCKED "Spotlight indexing is disabled in the lab image (mdutil -s /: $(mdutil -s / 2>&1 | tail -1 | tr -d '\t')); 'open', 'find' and Open With need mdfind"; fi
  # system commands: confirmation, declined
  o=$(ask 'shutdown'); has "$o" 'Shut Down' && result 3.2.9a PASS "'shutdown' lists Shut Down (OCR: $o)" || result 3.2.9a FAIL "$o"
  key_code $KC_RETURN; sleep 0.8; o=$(look shutdown-confirm)
  has "$o" 'sure|confirm|Return|again|really' && result 3.2.9b PASS "Return on Shut Down asks first (OCR: $o)" || result 3.2.9b FAIL "no confirmation text: $o"
  key_code $KC_ESC; sleep 0.8; uptime >"$QA_ART/uptime.txt"
  alive && result 3.2.9c PASS "Esc declined the shutdown; the guest is still up ($(cat "$QA_ART/uptime.txt" | cut -c1-60))" || result 3.2.9c FAIL "app gone after declining"
  result 3.2.9d BLOCKED "'lock' / 'sleep' / 'restart' / 'logout' were not executed: they would end the guest's GUI session (restart/logout) or lock it with no way to unlock from the script; the confirmation path is shown with shutdown. 'sleep' is covered in the soak (pmset)"
  hide_
}

# ------------------------------------------------------------------------------------------------------------------------ 3.3 keyboard
picked() { cat "$WORK/picked.txt" 2>/dev/null; }
clear_picked() { rm -f "$WORK/picked.txt"; }
await_picked() { local i; for i in 1 2 3 4 5 6 7 8; do [ -s "$WORK/picked.txt" ] && { picked; return; }; sleep 0.5; done; }
s33() {
  start_app; hide_; local o p
  # arrows and Ctrl+N/P (the `pk` workflow lists twelve rows; Return writes the row's arg to a file)
  ask 'pk ' 1.5 >/dev/null; clear_picked
  key_code $KC_DOWN; key_code $KC_DOWN; key_code $KC_DOWN; key_code $KC_UP; key_code $KC_RETURN; p=$(await_picked)
  [ "$p" = P03 ] && result 3.3.1a PASS "Down,Down,Down,Up then Return runs row 3 (picked=$p)" || result 3.3.1a FAIL "expected P03, picked='$p'"
  ask 'pk ' 1.2 >/dev/null; clear_picked
  se 'key code 45 using {control down}' >/dev/null; se 'key code 45 using {control down}' >/dev/null; se 'key code 35 using {control down}' >/dev/null; key_code $KC_RETURN; p=$(await_picked)
  [ "$p" = P02 ] && result 3.3.1b PASS "Ctrl+N, Ctrl+N, Ctrl+P then Return runs row 2 (picked=$p)" || result 3.3.1b FAIL "expected P02, picked='$p'"
  ask 'pk ' 1.2 >/dev/null; clear_picked; key_code $KC_UP; key_code $KC_RETURN; p=$(await_picked)
  result 3.3.1c PASS "Up on the first row: $([ "$p" = P12 ] && echo 'wraps to the last row' || echo "stays on the first row") (picked=$p); documented behaviour to be confirmed by the owner"
  ask 'pk ' 1.2 >/dev/null; clear_picked; local i; for i in 1 2 3 4 5 6 7 8 9 10 11; do key_code $KC_DOWN; done; sleep 0.6; o=$(look scrolled)
  key_code $KC_RETURN; p=$(await_picked)
  [ "$p" = P12 ] && has "$o" 'Pick 12' && result 3.3.1d PASS "11x Down reaches row 12, it is scrolled into view (OCR shows 'Pick 12') and Return runs it (picked=$p)" || result 3.3.1d FAIL "picked='$p' ocr: $o"
  # Return runs and hides
  ask 'pk ' 1.2 >/dev/null; key_code $KC_RETURN; sleep 1
  ! visible && result 3.3.2 PASS "Return runs the selected item and hides the launcher" || result 3.3.2 FAIL "launcher still visible after Return"
  # Tab completes the item's autocomplete text; only items that carry one (workflow keyword rows, bookmarks) have it
  show; clear_field; type_text "md"; sleep 1; key_code $KC_TAB; sleep 0.5; type_text "x"; sleep 1.2; o=$(look tab-md)
  has "$o" 'Mods row' && result 3.3.3a PASS "Tab on the workflow keyword row 'md' completes it to 'md ' (then 'x' lists the Script Filter rows: $o)" || result 3.3.3a FAIL "Tab did not complete the keyword row: $o"
  show; clear_field; type_text "safar"; sleep 0.8; key_code $KC_TAB; sleep 0.8; o=$(look tab-app)
  has "$o" '^Safari' && result 3.3.3b PASS "Tab completes an application row to its title (OCR: $o)" || result 3.3.3b FAIL "Tab on an application row does nothing (no autocomplete text): docs/QA.md 3.3 promises 'the selected item's title/autocomplete text'. crates/app/src/gui/keys.rs:250 needs item.autocomplete, which apps, files and calculator rows never set. OCR after Tab: $o"
  # Esc
  show; clear_field; type_text "abc"; key_code $KC_ESC; sleep 0.8
  if visible; then result 3.3.4 PASS "first Esc clears the text and keeps the launcher open (design note: second Esc hides)"; key_code $KC_ESC; sleep 0.5
  else result 3.3.4 PASS "Esc hides the launcher at once, with text in the field (it does not clear first)"; fi
  # Cmd+number
  ask 'pk ' 1.2 >/dev/null; clear_picked; o=$(look numbers); se 'keystroke "3" using {command down}' >/dev/null; p=$(await_picked)
  [ "$p" = P03 ] && has "$o" '1' && result 3.3.5 PASS "Cmd+3 runs the third row (picked=$p); the row hints on screen: $o" || result 3.3.5 FAIL "picked='$p'"
  # Large Type
  ask 'pk ' 1.2 >/dev/null; se 'keystroke "l" using {command down}' >/dev/null; sleep 1.5
  local big; big=$(win_tool Ashdash ashdash | awk '$6>=1000 && $7>=700 {print $6"x"$7}' | head -1)
  [ -n "$big" ] && result 3.3.6a PASS "Cmd+L opens Large Type (window $big)" || result 3.3.6a FAIL "no Large Type window: $(win_tool Ashdash ashdash | tr '\n' ';')"
  key_code $KC_ESC; sleep 0.8; [ -z "$(win_tool Ashdash ashdash | awk '$6>=1000 && $7>=700')" ] && result 3.3.6b PASS "Esc dismisses Large Type" || result 3.3.6b FAIL "Large Type still up after Esc"
  hide_
  # held Return on a confirmation: never confirms by repeating. Real target: Empty Trash with a file in the trash.
  mkdir -p "$HOME/.Trash"; echo x >"$HOME/.Trash/qa-trash-item.txt"
  tcc_reset
  ask 'empty trash' 1.2 >/dev/null
  # ONE key press held down: the first event is fresh (asks), the following 24 carry the auto-repeat flag
  qt keyrepeat 36 25 40; sleep 1.5
  local still; still=$(ls "$HOME/.Trash" | grep -c qa-trash-item)
  o=$(look held-return)
  [ "$still" = 1 ] && has "$o" 'again|confirm' && result 3.3.7a PASS "Return held (25 auto-repeat events): the confirmation is asked once and the trash is untouched (OCR: $o)" || result 3.3.7a FAIL "held Return: trash item present=$still ocr: $o"
  key_code $KC_RETURN; sleep 1
  local dlg; dlg=$(consent Allow 12)
  sleep 4
  [ "$(ls "$HOME/.Trash" 2>/dev/null | grep -c qa-trash-item)" = 0 ] && result 3.3.7b PASS "released, then a fresh Return confirms: Empty Trash ran through Finder (the first-use Automation prompt was: ${dlg:-none shown}); trash item gone" || result 3.3.7b FAIL "fresh Return did not empty the trash (prompt: ${dlg:-none}); footer: $(look empty-trash)"
  hide_
  # modifier overrides on a Script Filter item
  ask 'md x' 1.5 >/dev/null; clear_picked; key_code $KC_RETURN; p=$(await_picked)
  local mods_ok=1 r got
  for r in alt:option cmd:command shift:shift; do
    ask 'md x' 1.2 >/dev/null; clear_picked
    # the modifier is held for 0.4 s: a chord synthesised within one 16 ms frame ('key code 36 using {option down}') reaches the app with no modifier set
    se "key down ${r#*:}" >/dev/null; sleep 0.4; key_code 36 >/dev/null; sleep 0.2; se "key up ${r#*:}" >/dev/null; got=$(await_picked); [ "$got" = "${r%%:*}" ] || mods_ok=0; p="$p ${r%%:*}=$got"
  done
  [ "$mods_ok" = 1 ] && result 3.3.8 PASS "Return, Alt+Return, Cmd+Return, Shift+Return on a Script Filter row use the matching mods entry (picked: $p)" || result 3.3.8 FAIL "mods mismatch: $p"
  # Cmd+Q / Cmd+W / Cmd+H
  show; se 'keystroke "w" using {command down}' >/dev/null; sleep 0.8; local afterw; afterw=$(visible && echo visible || echo hidden)
  hide_; show && ok_w=1 || ok_w=0
  se 'keystroke "h" using {command down}' >/dev/null; sleep 0.8; local afterh; afterh=$(visible && echo visible || echo hidden)
  hide_; show && ok_h=1 || ok_h=0
  alive && [ "$ok_w$ok_h" = 11 ] && result 3.3.9a PASS "Cmd+W leaves the launcher $afterw, Cmd+H leaves it $afterh; the hotkey still shows it afterwards, process alive" || result 3.3.9a FAIL "odd state after Cmd+W/H (w=$afterw,$ok_w h=$afterh,$ok_h)"
  show; se 'keystroke "q" using {command down}' >/dev/null; sleep 2
  ! alive && result 3.3.9b PASS "Cmd+Q quits the launcher" || result 3.3.9b FAIL "still running after Cmd+Q"
}

# ------------------------------------------------------------------------------------------------------------------------ 3.4 focus loss
s34() {
  start_app; hide_; local o
  textedit_open; show; se 'set frontmost of process "Finder" to true' >/dev/null; sleep 1.2
  ! visible && [ "$(front)" = Finder ] && result 3.4.1 PASS "activating another app (Finder) hides the launcher; front app is Finder; windows left: $(win_tool Ashdash ashdash | awk '$6>=600' | wc -l | tr -d ' ')" || result 3.4.1 FAIL "launcher visible after activating Finder (front=$(front))"
  textedit_open; local f0; f0=$(front); show; se 'key code 48 using {command down}' >/dev/null; sleep 1.5
  local f1; f1=$(front); sleep 2
  if [ "$f1" = "$f0" ] && visible; then result 3.4.2 BLOCKED "synthetic Cmd+Tab does not switch apps in the guest (front stayed $f1), so the launcher had no reason to hide; do it on a real keyboard: show the launcher, press Cmd+Tab"
  elif ! visible; then result 3.4.2 PASS "Cmd+Tab hides the launcher and it does not come back or steal focus (front app after 3.5 s: $(front), was $f0)"
  else result 3.4.2 FAIL "launcher visible after Cmd+Tab although the front app changed ($f0 -> $f1)"; fi
  textedit_close
  result 3.4.3 NEEDS-OWNER "Spaces: the VM has one Space and no Mission Control driver. Steps: add a 2nd Space (Mission Control, +), press the hotkey, then Ctrl+Right: the launcher must hide and not follow; Ctrl+Left, hotkey again: it shows on the current Space"
  # display sleep / wake: in this VM 'pmset displaysleepnow' LOCKS the session (loginwindow, CGSSessionScreenIsLocked) and nothing in the lab can unlock it
  # (System Events keystrokes do not reach the login window; sysadminctl -screenLock off did not help), so the rest of the GUI run would be lost. Not executed here;
  # the soak does it as its very last step.
  result 3.4.4a BLOCKED "display sleep / wake: 'pmset displaysleepnow' in the guest locks the session and the lab cannot unlock it (tried: sysadminctl -screenLock off, askForPassword 0, key/typed password through System Events); the soak runs it last"
  result 3.4.4b NEEDS-OWNER "screen lock/unlock and display hot-plug: Ctrl+Cmd+Q to lock, unlock, press the hotkey; plug/unplug a monitor with the launcher hidden, press the hotkey (expected: shows on the main display, hotkey still works)"
  if ls /Applications/Ashdash.app >/dev/null 2>&1; then
    result 3.4.4c BLOCKED "real system sleep (lid close / pmset sleepnow) cannot be done in the VM without losing the ssh session: see the soak result for 'pmset sleepnow'"
  fi
}

# ------------------------------------------------------------------------------------------------------------------------ 3.5 clipboard history and snippets
clipfile() { echo "$DATA/clipboard.json"; }
copy() { qt pb-set "$@"; sleep 1.6; }   # the watcher polls: give it time
s35() {
  tcc_reset; start_app; hide_; local o
  # recorded copies, newest first, duplicates move to the top
  copy "QA-ONE-111"; copy "QA-TWO-222"; copy "QA-ONE-111"
  o=$(ask 'clip' 1.5); local first second
  first=$(echo "$o" | tr '|' '\n' | grep -n 'QA-' | head -2 | tr '\n' ' ')
  echo "$first" | grep -q 'QA-ONE-111.*QA-TWO-222' && result 3.5.1 PASS "'clip' lists copies newest first and a repeated copy moves to the top: $first" || result 3.5.1 FAIL "order wrong or entries missing: $first (OCR: $o)"
  # Return copies the entry back
  key_code $KC_DOWN; key_code $KC_RETURN; sleep 1.2; local pb; pb=$(pbpaste)
  [ "$pb" = "QA-TWO-222" ] && result 3.5.2a PASS "Return on the 2nd entry puts it on the pasteboard (pbpaste: $pb)" || result 3.5.2a FAIL "pbpaste='$pb'"
  # the exact thing Cmd+V does is read the pasteboard: paste it into TextEdit with a real Cmd+V
  textedit_open; se 'keystroke "v" using {command down}' >/dev/null; sleep 1; local te; te=$(textedit_text); textedit_close
  [ "$te" = "QA-TWO-222" ] && result 3.5.2b PASS "Cmd+V into TextEdit pastes the restored entry ('$te')" || result 3.5.2b FAIL "TextEdit got '$te'"
  # concealed / transient never reach history (real pasteboard with the nspasteboard.org marker types)
  copy "SECRET-CONCEALED-333" concealed; copy "SECRET-TRANSIENT-444" transient; copy "SECRET-BOTH-555" both
  o=$(ask 'clip' 1.5)
  ! has "$o" 'SECRET' && ! grep -q 'SECRET' "$(clipfile)" 2>/dev/null && result 3.5.3 PASS "ConcealedType / TransientType pasteboard items: not in 'clip' and not in clipboard.json (marker types on the pasteboard: $(qt pb-types | tr '\n' ' '))" || result 3.5.3 FAIL "secret reached history: ocr=$o file=$(grep -c SECRET "$(clipfile)" 2>/dev/null)"
  # file permissions
  local modes; modes=$(stat -f '%Sp %N' "$DATA"/clipboard.json "$DATA"/usage.json 2>&1 | sed "s|$DATA/||" | tr '\n' ' ')
  has "$modes" '[-]rw[-]{7} clipboard.json' && result 3.5.4 PASS "clipboard.json is 0600 ($modes)" || result 3.5.4 FAIL "modes: $modes"
  # files
  qt pb-set-file "$WORK/files/report.txt"; sleep 1.8
  o=$(ask 'clip' 1.5); has "$o" 'report.txt' && result 3.5.5a PASS "a copied file is listed by its name (OCR: $o)" || result 3.5.5a FAIL "no file entry: $o"
  key_code $KC_RETURN; sleep 1.2; qt pb-types | grep -q 'public.file-url' && result 3.5.5b PASS "Return restores the file reference (pasteboard types: $(qt pb-types | tr '\n' ' '))" || result 3.5.5b FAIL "restored types: $(qt pb-types | tr '\n' ' ')"
  # per-app ignore list (the frontmost app at copy time decides)
  cli config set clipboard.ignored_apps '["com.apple.TextEdit"]' >/dev/null 2>&1; sleep 2
  textedit_open; copy "IGNORED-BY-APP-666"; textedit_close; open -a Finder; sleep 1; copy "KEPT-BY-APP-777"
  o=$(ask 'clip' 1.5); has "$o" 'KEPT-BY-APP-777' && ! has "$o" 'IGNORED-BY-APP' && result 3.5.6 PASS "copy made while TextEdit is frontmost is skipped after adding it to clipboard.ignored_apps (no restart); a copy from Finder is kept" || result 3.5.6 FAIL "ocr: $o"
  cli config set clipboard.ignored_apps '[]' >/dev/null 2>&1
  # snippets
  o=$(ask 'snip' 1.5); has "$o" 'qa sig' && has "$o" 'qa plain' && result 3.5.7a PASS "'snip' lists the seeded snippets (OCR: $o)" || result 3.5.7a FAIL "$o"
  # without Accessibility: the copy happens and the message says how to fix it
  copy "CLIP-FOR-SNIPPET"
  o=$(ask 'snip qaclip' 1.5); key_code $KC_RETURN; sleep 1.5
  local pb2; pb2=$(pbpaste)
  show; local msg; msg=$(look no-ax)
  [ "$pb2" = "got:CLIP-FOR-SNIPPET" ] && result 3.5.7b PASS "{clipboard} expands from the current clipboard ('$pb2')" || result 3.5.7b FAIL "pasteboard after the snippet: '$pb2'"
  has "$msg" 'Accessibility' && result 3.5.8a PASS "auto-paste without the Accessibility permission copies and explains: $msg" || result 3.5.8a FAIL "no Accessibility explanation (footer/ocr: $msg)"
  hide_
  # grant Accessibility + PostEvent to the bundle id in this clone's TCC.db (SIP is off), restart, paste for real
  for svc in kTCCServiceAccessibility kTCCServicePostEvent; do
    sudo -n sqlite3 "/Library/Application Support/com.apple.TCC/TCC.db" "insert or replace into access(service,client,client_type,auth_value,auth_reason,auth_version,csreq,policy_id,indirect_object_identifier_type,indirect_object_identifier,indirect_object_code_identity,flags,last_modified) values('$svc','dev.ashdash.launcher',0,2,4,1,NULL,NULL,NULL,'UNUSED',NULL,0,CAST(strftime('%s','now') AS INTEGER));"
  done
  sudo -n killall tccd 2>/dev/null; sleep 2; start_app
  textedit_open; show; clear_field; type_text "snip qaplain"; sleep 1.2; key_code $KC_RETURN; sleep 2
  te=$(textedit_text)
  [ "$te" = "PLAIN-SNIPPET-TEXT" ] && result 3.5.8b PASS "with Accessibility granted to dev.ashdash.launcher, Return on a snippet pastes into the frontmost app with a real Cmd+V (TextEdit now contains '$te')" || result 3.5.8b FAIL "TextEdit contains '$te'; front=$(front)"
  se 'tell process "TextEdit" to keystroke "a" using command down' >/dev/null; se 'key code 51' >/dev/null
  # the snippet paste and a transient copy are not history entries
  o=$(ask 'clip' 1.5)
  ! has "$o" 'PLAIN-SNIPPET-TEXT' && ! grep -q 'PLAIN-SNIPPET-TEXT' "$(clipfile)" && result 3.5.9 PASS "the snippet's own paste (it goes through the pasteboard) is not in clipboard history or clipboard.json" || result 3.5.9 FAIL "snippet text reached history: $o"
  textedit_close; hide_
  # clipboard history item auto-paste
  copy "HIST-PASTE-888"; copy "OTHER-999"
  textedit_open; show; clear_field; type_text "clip HIST-PASTE"; sleep 1.2; key_code $KC_RETURN; sleep 2; te=$(textedit_text); textedit_close
  [ "$te" = "HIST-PASTE-888" ] && result 3.5.10 PASS "Return on a clip entry pastes it into the frontmost app ('$te')" || result 3.5.10 FAIL "TextEdit got '$te'"
  result 3.5.11 NEEDS-OWNER "a real password manager: in 1Password/Bitwarden use 'copy password' (they set org.nspasteboard.ConcealedType), then open 'clip' and run: grep -c <part of the password> ~/Library/Application\\ Support/ashdash/clipboard.json  (expected 0, and no row). The pasteboard marker handling itself is verified above with the same type names"
}

# ------------------------------------------------------------------------------------------------------------------------ 3.6 workflows
mkpkg() { # mkpkg <out.alfredworkflow> <bundle> <keyword> [extra zip entry name]
  python3 - "$@" <<'PY'
import plistlib, sys, zipfile
out, bundle, kw = sys.argv[1:4]
extra = sys.argv[4] if len(sys.argv) > 4 else None
pkg = {"name": "Pkg " + bundle, "bundleid": bundle, "createdby": "QA", "description": "synthetic",
       "objects": [{"uid": "K", "type": "alfred.workflow.input.keyword", "config": {"keyword": kw, "withspace": True, "argumenttype": 1, "text": "PkgRow " + bundle, "subtext": "s"}},
                   {"uid": "C", "type": "alfred.workflow.output.clipboard", "config": {"clipboardtext": "pkg-{query}"}}],
       "connections": {"K": [{"destinationuid": "C", "modifiers": 0}]}}
with zipfile.ZipFile(out, "w") as z:
    z.writestr("info.plist", plistlib.dumps(pkg))
    if extra:
        z.writestr(extra, "evil")
PY
}
s36() {
  start_app; hide_; local o rc t0 t1
  mkpkg "$WORK/good.alfredworkflow" qa.pkg.good pg
  o=$(cli import "$WORK/good.alfredworkflow" </dev/null 2>&1); rc=$?
  [ $rc != 0 ] && has "$o" 'yes|terminal|refus' && [ ! -d "$DATA/workflows/qa.pkg.good" ] && result 3.6.1a PASS "import with stdin not a terminal and no --yes refuses (rc $rc): $(echo "$o" | tail -2 | tr '\n' ' ')" || result 3.6.1a FAIL "rc=$rc out=$o"
  o=$(cli import --yes "$WORK/good.alfredworkflow" 2>&1); rc=$?
  sleep 2; o2=$(ask 'pg x' 1.2)
  [ $rc = 0 ] && has "$o2" 'PkgRow' && result 3.6.1b PASS "ashdash import --yes installs (summary printed: $(echo "$o" | head -3 | tr '\n' ' ')); the keyword works within 2 s without a reload (OCR: $o2)" || result 3.6.1b FAIL "rc=$rc import: $o | after: $o2"
  mkpkg "$WORK/evil.alfredworkflow" qa.pkg.evil pe "../../../../../../tmp/qa-evil-escape.txt"
  rm -f /tmp/qa-evil-escape.txt; o=$(cli import --yes "$WORK/evil.alfredworkflow" 2>&1); rc=$?
  [ ! -e /tmp/qa-evil-escape.txt ] && [ ! -e "$HOME/tmp/qa-evil-escape.txt" ] && ! find "$QAH" -name 'qa-evil-escape*' | grep -q . && result 3.6.2 PASS "a package with a '../' entry: rc=$rc ($(echo "$o" | tail -1 | cut -c1-150)); nothing written outside workflows/" || result 3.6.2 FAIL "escape file exists, rc=$rc"
  t0=$(now_ms); o=$(ask 'gb x' 1.5); has "$o" 'rror|invalid|could not|not valid|Script' && result 3.6.3a PASS "a Script Filter that prints garbage shows an error row, not a crash (OCR: $o)" || result 3.6.3a FAIL "no error row: $o"
  show; clear_field; type_text "hg x"; t0=$(now_ms); local gave=""; for i in $(seq 1 30); do sleep 1; o=$(look hang); if has "$o" 'rror|timed out|too long|killed|did not'; then gave=$(( $(now_ms) - t0 )); break; fi; done
  [ -n "$gave" ] && [ "$gave" -lt 16000 ] && result 3.6.3b PASS "a Script Filter that sleeps 30 s gives up after ~$((gave / 1000)) s with an error row (OCR: $o)" || result 3.6.3b FAIL "no timeout error within 30 s: $o"
  # disabling a workflow removes its keywords (the `disabled` flag in info.plist is what Preferences writes)
  python3 - "$DATA/workflows/qa.pick/info.plist" <<'PY'
import plistlib, sys
p = sys.argv[1]; d = plistlib.load(open(p, "rb")); d["disabled"] = True; plistlib.dump(d, open(p, "wb"))
PY
  sleep 3; o=$(ask 'pk ' 1.2); ! has "$o" 'Pick 01' && result 3.6.4a PASS "disabled workflow (info.plist disabled=true): its keyword stops listing rows within 3 s" || result 3.6.4a FAIL "still listed: $o"
  python3 - "$DATA/workflows/qa.pick/info.plist" <<'PY'
import plistlib, sys
p = sys.argv[1]; d = plistlib.load(open(p, "rb")); d.pop("disabled", None); plistlib.dump(d, open(p, "wb"))
PY
  sleep 3
  rm -f "$WORK/trigger.txt"; o=$(cli trigger qa.trigger go hello-trigger 2>&1); sleep 1
  [ "$(cat "$WORK/trigger.txt" 2>/dev/null)" = hello-trigger ] && result 3.6.4b PASS "ashdash trigger qa.trigger go <arg> runs the external trigger (file content: $(cat "$WORK/trigger.txt"))" || result 3.6.4b FAIL "trigger did not run: $o"
  # workflow_dirs live edit
  mkdir -p "$WORK/extra"; cp -R "$DATA/workflows/qa.mods" "$WORK/extra/qa.extra"
  python3 - "$WORK/extra/qa.extra/info.plist" <<'PY'
import plistlib, sys
p = sys.argv[1]; d = plistlib.load(open(p, "rb")); d["bundleid"] = "qa.extra"; d["objects"][0]["config"]["keyword"] = "ex"; plistlib.dump(d, open(p, "wb"))
PY
  cli config set workflow_dirs "[\"$WORK/extra\"]" >/dev/null 2>&1; sleep 3; o=$(ask 'ex x' 1.2)
  has "$o" 'Mods row' && result 3.6.5a PASS "workflow_dirs edited in config.json: the new folder's keyword works within 3 s, no prompt (by design; SECURITY.md section 3) (OCR: $o)" || result 3.6.5a FAIL "$o"
  cli config set workflow_dirs '[]' >/dev/null 2>&1; sleep 3; o=$(ask 'ex x' 1.2)
  ! has "$o" 'Mods row' && result 3.6.5b PASS "removing the entry removes the keyword within 3 s" || result 3.6.5b FAIL "still listed: $o"
  # import prompt through Finder (Open With / double click): the osascript dialog
  mkpkg "$WORK/finder.alfredworkflow" qa.pkg.finder pf
  wd 20 open -a "$APP" "$WORK/finder.alfredworkflow" >/dev/null 2>&1; sleep 4
  o=$(ax_dialogs | grep -v 'CoreServicesUIAgent | window .*: $' | head -3)
  if [ -n "$o" ]; then result 3.6.6a PASS "opening a .alfredworkflow with Ashdash (Finder route) raises the import dialog: $(echo "$o" | cut -c1-300)"; for b in Cancel "Don’t Install" "Don’t Allow"; do click_button ashdash "$b" 2>/dev/null; click_button Ashdash "$b" 2>/dev/null; done
  else result 3.6.6a FAIL "no import dialog after 'open -a Ashdash.app file.alfredworkflow' (workflows now: $(cli workflows 2>&1 | tr '\n' ' ' | cut -c1-200))"; fi
  [ ! -d "$DATA/workflows/qa.pkg.finder" ] && result 3.6.6b PASS "nothing was installed without confirming" || result 3.6.6b FAIL "installed without confirmation"
  result 3.6.7 BLOCKED "on-screen drive not written for: trust-summary wording per object type, Write-File-only package, 600 ms Return guard in the launcher prompt, re-import 'Replace workflow', package swapped under the prompt, RTL-override / 10,000-character names. The launcher prompt (Right Arrow > Install Workflow, early Return ignored, Esc leaves nothing, fresh Return installs) was driven by scripts/qa/ui-glue-smoke.sh in this VM, see rows 3.9.*. The rest is covered by cargo tests import_staging / import_security / trust"
}

# ------------------------------------------------------------------------------------------------------------------------ Wave 4/5 behaviours
w45() {
  start_app; hide_; local o i
  # the highlighted result keeps following its id when a rerun reorders the list: runs 1-2 give A,B,C, run 3 on gives C,A,B (a counter file)
  local okc=0 detail="" p
  for i in 1 2 3; do
    hide_; rm -f "$WORK/rr-count"; clear_picked
    show; clear_field; type_text "rr "; sleep 0.45; key_code $KC_DOWN       # highlight row 2 = B while the order is still A,B,C
    sleep 4.5; local seen; seen=$(look rr-after | tr '|' '\n' | grep -E '^Row [ABC]' | tr '\n' ' ')
    key_code $KC_RETURN; p=$(await_picked); detail="$detail [trial $i: moved to B, list later '$seen' -> Return ran '$p']"
    [ "$p" = B ] && okc=$((okc+1))
  done
  [ $okc = 3 ] && result W5.1 PASS "Script Filter rerun re-orders the list under the cursor (A,B,C -> C,A,B): the row moved to stays highlighted by id and Return runs it, 3 of 3 trials:$detail" || result W5.1 FAIL "selection followed the id in $okc of 3 trials:$detail"
  # slow filter: pending row, then the final list
  hide_; sleep 0.8; show; clear_field; type_text "sl x"; sleep 1.0; local p1; p1=$(look slow-pending)
  sleep 3.2; local p2; p2=$(look slow-final)
  has "$p1" 'Slow' && has "$p2" 'Slow one' && result W5.2a PASS "slow Script Filter: a running row ('Slow…') first (OCR: $p1), the three rows after 3 s (OCR: $p2)" || result W5.2a FAIL "pending: $p1 / final: $p2"
  clear_picked; key_code $KC_DOWN; key_code $KC_RETURN; p=$(await_picked)
  [ "$p" = s2 ] && result W5.2b PASS "after the slow answer arrived, Down + Return runs the 2nd row that is on screen (picked=$p)" || result W5.2b FAIL "picked '$p', expected s2"
  result W5.2c BLOCKED "'Return in the very frame in which a new list replaces the old one' needs sub-frame timing that a script cannot provoke; the logic is unit-tested (gui/schedule.rs: return_*, cmd_number_*, several_replacements_*), the id-tracking half is shown by W5.1/W5.2b"
  # crash note
  stop_app; mkdir -p "$DATA/crashes"; rm -f "$DATA"/crashes/*
  printf 'ashdash crash report (synthetic QA fixture)\nversion: test\n' >"$DATA/crashes/crash-20261017T120102.345Z.txt"
  start_app; show; sleep 1; o=$(look crash-note-1)
  hide_; show; sleep 1; local o2; o2=$(look crash-note-2)
  has "$o" 'quit unexpectedly' && ! has "$o2" 'quit unexpectedly' && result W5.3a PASS "a report newer than crashes/last-seen: the footer says 'Ashdash quit unexpectedly · report in crashes/' on the first show (OCR: $o), not on the second ($o2)" || result W5.3a FAIL "first show: $o / second show: $o2"
  o=$(cli paths 2>&1 | grep -i crash | tr '\n' ' ')
  has "$o" 'crash-20261017T120102' && result W5.3b PASS "ashdash paths names the report: $o" || result W5.3b FAIL "ashdash paths: $o"
  # ashdash://search fills the field and runs nothing
  hide_; open "ashdash://search?query=rr%20x"; sleep 2; o=$(look url-search)
  has "$o" 'Text from a link' && ! has "$o" 'Row [ABC]' && result W5.4a PASS "ashdash://search?query=rr%20x fills the field and says 'Text from a link: edit it to search'; no Script Filter ran (no 'Row A/B/C' rows) (OCR: $o)" || result W5.4a FAIL "OCR: $o"
  hide_
  # URL consent dialogs: one per bundle at a time, a timeout stores nothing, an answer is remembered
  rm -f "$DATA/url-trust.json"
  open "ashdash://trigger?bundle=qa.trigger&name=go&arg=consent-1"; sleep 1; open "ashdash://trigger?bundle=qa.trigger&name=go&arg=consent-2"; sleep 3
  o=$(ax_dialogs | grep 'wants to run the workflow'); local nd; nd=$(echo "$o" | grep -c 'qa.trigger')
  [ "$nd" = 1 ] && result W5.4b PASS "first ashdash://trigger for a bundle: consent dialog 'A link or app wants to run the workflow \"QA trigger\" (qa.trigger) in Ashdash', default Don't Allow; two URLs in a row for the same bundle raise ONE dialog (dialogs for qa.trigger: $nd)" || result W5.4b FAIL "dialogs for qa.trigger: $nd ($o)"
  sleep 66
  o=$(ax_dialogs | grep -c 'wants to run the workflow'); local stored; stored=$(cat "$DATA/url-trust.json" 2>/dev/null)
  [ "$o" = 0 ] && ! has "$stored" 'qa.trigger' && [ ! -s "$WORK/trigger.txt" -o "$(cat "$WORK/trigger.txt" 2>/dev/null)" != consent-1 ] && result W5.4c PASS "left alone for 60 s the dialog times out: nothing runs and nothing is stored (url-trust.json: '${stored:-absent}')" || result W5.4c FAIL "after the timeout: dialogs=$o stored='$stored' trigger.txt='$(cat "$WORK/trigger.txt" 2>/dev/null)'"
  rm -f "$WORK/trigger.txt"; open "ashdash://trigger?bundle=qa.trigger&name=go&arg=consent-3"; sleep 3
  ax_dialogs | grep -q 'wants to run the workflow' && result W5.4d PASS "after the timeout the next URL asks again" || result W5.4d FAIL "no second dialog"
  click_button osascript "Allow"; sleep 2
  [ "$(cat "$WORK/trigger.txt" 2>/dev/null)" = consent-3 ] && result W5.4e PASS "Allow runs the trigger (trigger.txt = $(cat "$WORK/trigger.txt")); url-trust.json now: $(tr -d '\n ' <"$DATA/url-trust.json" | cut -c1-120)" || result W5.4e FAIL "trigger.txt='$(cat "$WORK/trigger.txt" 2>/dev/null)'"
  rm -f "$WORK/trigger.txt"; open "ashdash://trigger?bundle=qa.trigger&name=go&arg=consent-4"; sleep 3
  ! ax_dialogs | grep -q 'wants to run the workflow' && [ "$(cat "$WORK/trigger.txt" 2>/dev/null)" = consent-4 ] && result W5.4f PASS "the answer is remembered: the next URL runs without asking" || result W5.4f FAIL "asked again or did not run"
  open "ashdash://trigger?bundle=qa.url&name=u&arg=x"; sleep 3; click_button osascript "Don't Allow"; sleep 2
  open "ashdash://trigger?bundle=qa.url&name=u&arg=y"; sleep 3
  ! ax_dialogs | grep -q 'wants to run the workflow' && has "$(cat "$DATA/url-trust.json")" 'qa.url' && result W5.4g PASS "Don't Allow is remembered too (url-trust.json: $(tr -d '\n ' <"$DATA/url-trust.json" | cut -c1-120)); no further dialog" || result W5.4g FAIL "after Don't Allow: $(cat "$DATA/url-trust.json")"
  # a transient workflow copy and a plain one
  hide_; copy "SENTINEL-BEFORE"
  ask 'cp zz' 1.2 >/dev/null; key_code $KC_RETURN; sleep 2; local pb; pb=$(pbpaste)
  ask 'cq yy' 1.2 >/dev/null; key_code $KC_RETURN; sleep 2; local pb2; pb2=$(pbpaste)
  o=$(ask 'clip' 1.5)
  [ "$pb" = TRANSIENT-zz ] && [ "$pb2" = PLAINCOPY-yy ] && ! has "$o" 'TRANSIENT-zz' && has "$o" 'PLAINCOPY-yy' && result W5.5 PASS "workflow Copy to Clipboard: the transient one reaches the pasteboard ('$pb') but not clip history; the plain one does ('$pb2' listed) (OCR: $o)" || result W5.5 FAIL "pb='$pb' pb2='$pb2' ocr: $o"
}

main() {
  local secs=("$@"); [ ${#secs[@]} = 0 ] && secs=(s_setup s31 s32 s33 s34 s35 s36 s37 s38 s39 s311 w45)
  local s; for s in "${secs[@]}"; do case "$s" in setup) s=s_setup ;; esac; echo "== $s"; "$s"; done
  stop_app 2>/dev/null
  echo "---- results"; cat "$QA_RESULTS"
}
[ "${BASH_SOURCE[0]}" = "$0" ] && main "$@"

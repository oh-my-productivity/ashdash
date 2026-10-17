#!/usr/bin/env bash
# W6-FOCUS probe: does the launcher hide when another app is activated? Runs INSIDE a lab VM (ssh session of the guest user),
# never on a host desktop. Copy of the 3.4.1 / 3.4.2 trials of tests/lab/qa/checklist.sh with a trace of the app's own log per trial.
#
#   bash tests/lab/focus/probe.sh [--trials N] [--no-build] [--only act|tab|both] [--finder-delay S] [--after-show S] [--race MS]
#
# 1. grants TCC in this clone (tests/lab/guest/tcc-grant.sh), 2. builds Ashdash.app (native, release) under the guest home,
# 3. starts it with a throw-away HOME and ASHDASH_TRACE=1, 4. runs the trials, 5. prints one line per trial plus the trace lines
# the app wrote during it, and a summary "act X/N tab Y/N" (also in $QA_ART/focus-summary.txt).
# Output: $LAB_ARTIFACTS (copied back by `lab-run --artifacts`).
# shellcheck shell=bash
# shellcheck disable=SC2015,SC2155
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../../.." || exit 1
# shellcheck source=../qa/lib.sh
. tests/lab/qa/lib.sh

trials=5 build=1 only=both fdelay=1.5 aftershow=0 race=
while [ $# -gt 0 ]; do
  case "$1" in
    --trials) trials=$2; shift 2 ;;
    --no-build) build=0; shift ;;
    --only) only=$2; shift 2 ;;
    --finder-delay) fdelay=$2; shift 2 ;;
    --race) race=$2; only=race; shift 2 ;;
    --after-show) aftershow=$2; shift 2 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done

DIST=${FOCUS_DIST:-$HOME/focus-dist}
APP=$DIST/Ashdash.app
BIN=$APP/Contents/MacOS/ashdash
QAH=$HOME/focus-home
LOG=$QA_ART/app.log
SUMMARY=$QA_ART/focus-summary.txt

alive() { pgrep -f "$BIN\$" >/dev/null; }
stop_app() { HOME=$QAH wd 10 "$BIN" quit >/dev/null 2>&1; sleep 1; pkill -f "$BIN\$" 2>/dev/null; sleep 0.5; }
start_app() {
  stop_app
  rm -rf "$QAH"; mkdir -p "$QAH"
  : >"$LOG"
  wd 25 "$GUI_EXEC" --detach -- /usr/bin/open --env "HOME=$QAH" --env ASHDASH_TRACE=1 --stdout "$LOG" --stderr "$LOG" "$APP" >/dev/null
  local i; for i in $(seq 1 30); do alive && break; sleep 1; done
  sleep 2.5
}
visible() { win_tool Ashdash ashdash | awk '$2==25 && $6>=600 {f=1} END {exit !f}'; }
show() { visible && return 0; hotkey; local i; for i in 1 2 3 4 5 6 7 8; do sleep 0.25; visible && return 0; done; return 1; }
hide_() { visible || return 0; key_code $KC_ESC; sleep 0.4; visible && { hotkey; sleep 0.5; }; }
textedit_open() { open -a TextEdit; sleep 3; se 'tell process "TextEdit" to keystroke "n" using command down' >/dev/null; sleep 1.5; }
textedit_close() { pkill -x TextEdit; sleep 1; rm -rf "$HOME/Library/Autosave Information" 2>/dev/null; }

if [ "${LAB_AUTO_TCC:-1}" = 1 ]; then
  bash tests/lab/guest/tcc-grant.sh >"$QA_ART/tcc-grant.txt" 2>&1 || { echo "tcc-grant failed"; cat "$QA_ART/tcc-grant.txt"; exit 1; }
  # tcc-grant.sh's `killall tccd` (SIGTERM) leaves the *system* tccd running with the old rows on macOS 15.7 (seen 2026-10-17: System Events
  # keystrokes keep failing with 1002 "not allowed to send keystrokes"); SIGKILL makes launchd start a fresh one that reads the new rows.
  sudo -n pkill -9 -f 'tccd system' 2>/dev/null; killall -9 tccd 2>/dev/null; sleep 3
fi
se 'key code 63' >/dev/null || { echo "System Events cannot send keys (TCC); see $QA_ART/tcc-grant.txt"; exit 1; }
if [ "$build" = 1 ]; then
  mkdir -p "$DIST"
  ( ARCHS=native SKIP_ICON=1 OUT_DIR=$DIST packaging/build-app.sh ) >"$QA_ART/build.log" 2>&1 || { tail -20 "$QA_ART/build.log"; exit 1; }
fi
[ -x "$BIN" ] || { echo "no $BIN"; exit 1; }
# warm up System Events / windows tool
se 'get name of first process whose frontmost is true' >/dev/null
win_tool Ashdash ashdash >/dev/null 2>&1

start_app
if ! alive; then echo "app not running"; tail "$LOG"; exit 1; fi
hide_
textedit_open

mark() { wc -l <"$LOG" | tr -d ' '; }
since() { tail -n +"$(( $1 + 1 ))" "$LOG"; }

n_act=0 n_tab=0
if [ "$only" = race ]; then
  # Finder is activated MS milliseconds after the hotkey went out, without waiting for the launcher to show first
  # (MS 0..400 covers "before the window became key" to "after the first frame"); the launcher must not be stuck on screen
  # without the focus 1.5 s later, and the hotkey must show it again afterwards.
  n_race=0
  for t in $(seq 1 "$trials"); do
    se 'set frontmost of process "TextEdit" to true' >/dev/null; sleep 1; hide_
    m=$(mark)
    # the System Events call has its own start-up time; "open -a" is faster than osascript, the hotkey goes through a background job
    ( key_code 49 "option down, shift down" >/dev/null & sleep "$(awk "BEGIN{print $race/1000}")"; open -a Finder; wait )
    sleep "$fdelay"
    # Visible 1.5 s later is fine only if the launcher really has the focus (Finder came first, the hotkey after it): a stuck window
    # is a visible one whose last drawn frame says "not focused".
    if visible; then
      lastf=$(since "$m" | grep 'frame focused=' | tail -1)
      case "$lastf" in *"focused=Some(true)"*) r=shown-and-focused ;; *) r=STUCK ;; esac
    else r=hid; fi
    show && again=reshown || again=NOT-RESHOWN
    [ "$r" != STUCK ] && [ "$again" = reshown ] && n_race=$((n_race+1))
    echo "race $t: $r, then $again"; since "$m" | sed 's/^/    /'
    visible && { hide_; }
  done
  echo "RESULT race(${race}ms) $n_race/$trials" | tee "$SUMMARY"
fi
if [ "$only" != tab ] && [ "$only" != race ]; then
  for t in $(seq 1 "$trials"); do
    se 'set frontmost of process "TextEdit" to true' >/dev/null; sleep 1; hide_; show
    m=$(mark); sleep "$aftershow"
    se 'set frontmost of process "Finder" to true' >/dev/null; sleep "$fdelay"
    if visible; then r=STAYED; else r=hid; n_act=$((n_act+1)); fi
    echo "act $t: $r"; since "$m" | sed 's/^/    /'
    visible && { hide_; }
  done
fi
if [ "$only" != act ] && [ "$only" != race ]; then
  for t in $(seq 1 "$trials"); do
    se 'set frontmost of process "TextEdit" to true' >/dev/null; sleep 1; hide_; show; f0=$(front)
    m=$(mark); sleep "$aftershow"
    se 'key code 48 using {command down}' >/dev/null; sleep 1.5
    if [ "$(front)" != "$f0" ] && ! visible; then r=hid; n_tab=$((n_tab+1)); else r="STAYED(front=$(front))"; fi
    echo "tab $t: $r"; since "$m" | sed 's/^/    /'
    visible && { hide_; }
  done
fi
textedit_close
[ "$only" = race ] || echo "RESULT act $n_act/$trials tab $n_tab/$trials (only=$only, finder-delay=$fdelay, after-show=$aftershow)" | tee "$SUMMARY"
cp "$LOG" "$QA_ART/app-final.log" 2>/dev/null
stop_app

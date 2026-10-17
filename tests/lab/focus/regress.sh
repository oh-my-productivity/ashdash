#!/usr/bin/env bash
# W6-FOCUS regression checks for the "hide on focus loss" rule, INSIDE a lab VM (see probe.sh for the setup it reuses):
#   * Quick Look (qlmanage) takes the focus and the launcher must stay, Esc in the preview gives the focus back
#   * actions panel and Esc: the panel opens, the launcher stays; Esc closes the panel, Esc hides the launcher
#   * a click inside the launcher keeps it up
#   * idle CPU: hidden for 60 s, and shown and focused for 30 s (CPU seconds used by the process, ps cputime)
#   bash tests/lab/focus/regress.sh [--no-build] [--only ql|panel|click|idle]    (the app is Ashdash.app from probe.sh's build, ~/focus-dist)
# shellcheck shell=bash
# shellcheck disable=SC2015,SC2155
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../../.." || exit 1
# shellcheck source=../qa/lib.sh
. tests/lab/qa/lib.sh

build=1 only=all
while [ $# -gt 0 ]; do case "$1" in --no-build) build=0; shift ;; --only) only=$2; shift 2 ;; *) echo "unknown option $1" >&2; exit 2 ;; esac; done
want() { [ "$only" = all ] || [ "$only" = "$1" ]; }
DIST=$HOME/focus-dist APP=$HOME/focus-dist/Ashdash.app
BIN=$APP/Contents/MacOS/ashdash QAH=$HOME/focus-home LOG=$QA_ART/app.log
alive() { pgrep -f "$BIN\$" >/dev/null; }
app_pid() { pgrep -f "$BIN\$" | head -1; }
stop_app() { HOME=$QAH wd 10 "$BIN" quit >/dev/null 2>&1; sleep 1; pkill -f "$BIN\$" 2>/dev/null; sleep 0.5; }
start_app() {
  stop_app; rm -rf "$QAH"; mkdir -p "$QAH"; : >"$LOG"
  wd 25 "$GUI_EXEC" --detach -- /usr/bin/open --env "HOME=$QAH" --env ASHDASH_TRACE=1 --stdout "$LOG" --stderr "$LOG" "$APP" >/dev/null
  local i; for i in $(seq 1 30); do alive && break; sleep 1; done; sleep 2.5
}
visible() { win_tool Ashdash ashdash | awk '$2==25 && $6>=600 {f=1} END {exit !f}'; }
# any launcher-sized window at layer 25 (raised) or 0 (a Quick Look preview is up and the launcher is lowered below it)
visible_any() { win_tool Ashdash ashdash | awk '($2==25 || $2==0) && $6>=600 {f=1} END {exit !f}'; }
show() { visible && return 0; hotkey; local i; for i in 1 2 3 4 5 6 7 8; do sleep 0.25; visible && return 0; done; return 1; }
hide_() { visible || return 0; key_code $KC_ESC; sleep 0.4; visible && { hotkey; sleep 0.5; }; }
cpu_s() { ps -o cputime= -p "$1" | awk -F'[:.]' '{ if (NF==3) print $1*60+$2+$3/100; else print $1*3600+$2*60+$3+$4/100 }'; }
has() { echo "$1" | grep -qiE "$2"; }
look() { shot "$1" 2>/dev/null || { echo "(no window)"; return 1; }; ocr "$QA_ART/$1.png" 2>/dev/null | grep -v IOServiceMatching | tr '\n' '|'; }
last_frame() { grep 'frame focused=' "$LOG" | tail -1; }

if [ "${LAB_AUTO_TCC:-1}" = 1 ]; then
  bash tests/lab/guest/tcc-grant.sh >"$QA_ART/tcc-grant.txt" 2>&1 || { echo "tcc-grant failed"; exit 1; }
  sudo -n pkill -9 -f 'tccd system' 2>/dev/null; killall -9 tccd 2>/dev/null; sleep 3
fi
se 'key code 63' >/dev/null || { echo "System Events cannot send keys (TCC)"; exit 1; }
if [ "$build" = 1 ]; then
  mkdir -p "$DIST"; ( ARCHS=native SKIP_ICON=1 OUT_DIR=$DIST packaging/build-app.sh ) >"$QA_ART/build.log" 2>&1 || { tail -20 "$QA_ART/build.log"; exit 1; }
fi
win_tool Ashdash >/dev/null 2>&1; start_app; alive || { echo "app not running"; exit 1; }
hide_
fails=0
check() { # check <name> <ok 0|1> <evidence>
  if [ "$2" = 1 ]; then echo "PASS $1: $3"; else echo "FAIL $1: $3"; fails=$((fails+1)); fi
}

# --- Quick Look: a file result, Cmd+Y starts qlmanage which takes the focus; the launcher stays up (lowered below the preview), then Esc in the preview
if want ql; then for i in 1 2 3; do
  show; chord command 0; key_code 51 >/dev/null  # select all, delete
  se 'keystroke "/etc/hosts"' >/dev/null; sleep 1.2
  chord command 16; sleep 2     # Cmd+Y
  ql=0; pgrep -x qlmanage >/dev/null && ql=1
  stay=0; visible_any && stay=1
  key_code $KC_ESC >/dev/null; sleep 1.5               # Esc goes to the frontmost app: the preview
  ql_gone=1; pgrep -x qlmanage >/dev/null && ql_gone=0
  back=0; visible && back=1
  foc=0; sleep 0.3; last_frame | grep -q 'focused=Some(true)' && foc=1
  check "quicklook-$i" $((ql & stay & ql_gone & back & foc)) "preview started=$ql, launcher stayed up=$stay, Esc ended the preview=$ql_gone, launcher raised again=$back and focused=$foc"
  hide_; pkill -x qlmanage; sleep 0.5
done; fi

# --- actions panel (Right arrow on a file result) and Esc: Esc closes the panel and keeps the launcher, the next Esc hides it
if want panel; then for i in 1 2 3; do
  show; chord command 0; key_code 51 >/dev/null
  se 'keystroke "/etc/hosts"' >/dev/null; sleep 1.2
  key_code $KC_RIGHT >/dev/null; sleep 1.5
  txt=$(look "panel-$i"); panel_up=0; has "$txt" 'Reveal|Copy|Open' && visible && panel_up=1
  key_code $KC_ESC >/dev/null; sleep 1
  txt1=$(look "panel-$i-esc1"); after_esc1=0; visible && after_esc1=1
  # (with text in the field one Esc clears it, and the next one hides: docs/QA.md 3.3.4)
  after_esc2=0; for _ in 1 2; do key_code $KC_ESC >/dev/null; sleep 1; visible || { after_esc2=1; break; }; done
  check "panel-$i" $((panel_up & after_esc1 & after_esc2)) "panel shown (OCR '$(echo "$txt" | cut -c1-80)')=$panel_up; after the first Esc the launcher is up=$after_esc1 ('$(echo "$txt1" | cut -c1-60)'); hidden within two more Esc=$after_esc2"
  hide_
done; fi

# --- a click inside the launcher keeps it (and its focus)
if want click; then
show; read -r x y w h < <(win_tool Ashdash ashdash | awk '$2==25 && $6>=600 {print $4, $5, $6, $7; exit}')
qt click $((x + w / 2)) $((y + h / 2)) >/dev/null 2>&1; sleep 1
c=0; visible && last_frame | grep -q 'focused=Some(true)' && c=1
check click-inside $c "launcher up and focused after a click in the window (${w}x${h} at $x,$y)"
hide_
fi

# --- idle CPU
if want idle; then
pid=$(app_pid); sleep 5; a=$(cpu_s "$pid"); sleep 60; b=$(cpu_s "$pid")
d=$(awk "BEGIN{printf \"%.2f\", $b-$a}"); ok=0; awk "BEGIN{exit !($d < 0.30)}" && ok=1
check idle-hidden-60s $ok "CPU seconds used in 60 s while hidden: $d (pid $pid)"
show; sleep 3; a=$(cpu_s "$pid"); sleep 30; b=$(cpu_s "$pid")
d=$(awk "BEGIN{printf \"%.2f\", $b-$a}"); ok=0; awk "BEGIN{exit !($d < 0.60)}" && ok=1
check idle-shown-30s $ok "CPU seconds used in 30 s while shown and focused: $d"
hide_
fi
echo "frames in the trace so far: $(grep -c 'frame focused=' "$LOG")"
stop_app
echo "REGRESS failures=$fails" | tee "$QA_ART/regress-summary.txt"
exit "$fails"

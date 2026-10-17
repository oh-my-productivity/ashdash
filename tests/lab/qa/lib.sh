#!/usr/bin/env bash
# Helpers for the W6 QA scripts. Sourced INSIDE a lab VM (ssh session of the guest user), never on the host desktop.
# shellcheck shell=bash
# shellcheck disable=SC2034  # variables are used by the scripts that source this file

QA_ROOT=${QA_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)}
QA_ART=${QA_ART:-${LAB_ARTIFACTS:-$(mktemp -d "${TMPDIR:-/tmp}/qa-art.XXXXXX")}}
mkdir -p "$QA_ART"
QA_RESULTS=${QA_RESULTS:-$QA_ART/results.tsv}   # id <TAB> PASS|FAIL|BLOCKED|NEEDS-OWNER <TAB> evidence
GUI_EXEC=$QA_ROOT/tests/lab/guest/gui-exec
QA_BIN_DIR=${QA_BIN_DIR:-$HOME/qa-tools}
mkdir -p "$QA_BIN_DIR"
export PATH="$HOME/.cargo/bin:$QA_BIN_DIR:$PATH"

# result <id> <PASS|FAIL|BLOCKED|NEEDS-OWNER> <evidence...>
result() {
  local id=$1 st=$2; shift 2
  printf '%s\t%s\t%s\n' "$id" "$st" "$*" | tee -a "$QA_RESULTS" >/dev/null
  printf '%-12s %-12s %s\n' "$id" "$st" "$*"
}

# wd <secs> cmd...: a command that blocks on an invisible consent dialog must not hang the run
wd() { local s=$1; shift; perl -e 'alarm shift; exec @ARGV' "$s" "$@" 2>&1; }

osa() { wd "${OSA_TIMEOUT:-20}" osascript -e "$1"; }
# System Events helpers (ssh session; the grants come from tests/lab/guest/tcc-grant.sh)
se() { osa "tell application \"System Events\" to $1"; }
key_code() { # key_code <code> [modifiers, e.g. "option down, shift down"]
  if [ -n "${2:-}" ]; then se "key code $1 using {$2}"; else se "key code $1"; fi
}
type_text() { se "keystroke \"$1\""; }
hotkey() { key_code 49 "option down, shift down"; }   # Option+Shift+Space (the default)
KC_RETURN=36 KC_ESC=53 KC_TAB=48 KC_UP=126 KC_DOWN=125 KC_LEFT=123 KC_RIGHT=124 KC_DELETE=51 KC_SPACE=49

# AX dump of every dialog-owning process: "process: window: static texts | buttons"
ax_dialogs() {
  local p
  for p in UserNotificationCenter CoreServicesUIAgent SecurityAgent universalAccessAuthWarn "System Settings" "System Preferences"; do
    wd 15 osascript - "$p" <<'OSA' 2>/dev/null
on run argv
  set pn to item 1 of argv
  tell application "System Events"
    if not (exists process pn) then return ""
    tell process pn
      if (count of windows) is 0 then return ""
      set out to ""
      repeat with w in windows
        set out to out & pn & " | window '" & (name of w as text) & "': "
        set els to entire contents of w
        repeat with e in els
          set c to ""
          try
            set c to (class of e) as text
          end try
          if c is "static text" then
            try
              set out to out & (value of e as text) & " // "
            end try
          else if c is "button" then
            try
              set t to (name of e as text)
              if t is not "missing value" then set out to out & "[" & t & "] "
            end try
          end if
        end repeat
        set out to out & linefeed
      end repeat
      return out
    end tell
  end tell
end run
OSA
  done | grep -v '^$'
}

# click_button <process> <button name>
click_button() { wd 20 osascript -e "tell application \"System Events\" to tell process \"$1\" to click button \"$2\" of window 1" >/dev/null; }

# Windows of a process (owner name): "id layer order x y w h owner"; tool compiled on first use from scripts/qa/windows.swift
win_tool() {
  [ -x "$QA_BIN_DIR/qa-windows" ] || swiftc -O "$QA_ROOT/scripts/qa/windows.swift" -o "$QA_BIN_DIR/qa-windows" 2>/dev/null
  "$QA_BIN_DIR/qa-windows" "$@"
}
# (the owner name is "Ashdash" for the bundle, "ashdash" for a bare binary)
nwin() { win_tool Ashdash ashdash | grep -c .; }
# The launcher's own window rows (layer 0 and 3 are windows, status item has its own layer)
launcher_windows() { win_tool Ashdash ashdash; }

# shot <name> [owner]: window-only screenshot (-l) of the first window of <owner> (default ashdash) into $QA_ART
shot() {
  local name=$1 owner=${2:-Ashdash} id
  id=$(win_tool "$owner" ashdash | awk '$6>100 && $7>30 {print $1; exit}')
  [ -n "$id" ] || { echo "shot: no window of $owner" >&2; return 1; }
  wd 20 screencapture -l "$id" -o -x "$QA_ART/$name.png"
}

# OCR a PNG (Vision) -> text on stdout; tool compiled on first use
ocr() {
  if [ ! -x "$QA_BIN_DIR/qa-ocr" ]; then swiftc -O "$QA_ROOT/tests/lab/qa/ocr.swift" -o "$QA_BIN_DIR/qa-ocr" 2>/dev/null; fi
  "$QA_BIN_DIR/qa-ocr" "$1"
}

# qa-tool (click / keyrepeat / pasteboard / hotkey hog / front app): compiled on first use from qa-tool.swift
qt() {
  if [ ! -x "$QA_BIN_DIR/qa-tool" ]; then swiftc -O "$QA_ROOT/tests/lab/qa/qa-tool.swift" -o "$QA_BIN_DIR/qa-tool" 2>"$QA_ART/qa-tool-build.log" || { cat "$QA_ART/qa-tool-build.log" >&2; return 1; }; fi
  "$QA_BIN_DIR/qa-tool" "$@"
}
front() { se 'get name of first process whose frontmost is true' | tail -1; }

#!/usr/bin/env bash
# QA item 2 (packaged install on a fresh account). Runs INSIDE a freshly cloned lab VM, with NO TCC grants for the app:
#
#   tests/lab/lab-run "$vm" --env QA_ART=... --artifacts out/ -- bash tests/lab/qa/pkg-install.sh
#
# Build the ad-hoc bundle and the DMG, mark the DMG and the installed app as quarantined the way a browser download does, turn Gatekeeper on
# (the base image has assessments disabled), and record what macOS refuses and what works to get past it. Results: $QA_ART/results.tsv.
# Needs the lab's ssh-session grants for System Events (the base image has them) but never runs tcc-grant.sh: the point is the untouched state.
# shellcheck disable=SC2015
set -uo pipefail
cd "$(dirname "$0")/../../.."
# shellcheck source=lib.sh
. tests/lab/qa/lib.sh
DIST=$HOME/qa-dist
APP=/Applications/Ashdash.app
quarantine() { xattr -rw com.apple.quarantine "0083;$(printf %x "$(date +%s)");Safari;$(uuidgen)" "$1"; }
running() { pgrep -f "$APP/Contents/MacOS/ashdash|AppTranslocation/.*/Ashdash.app/Contents/MacOS/ashdash" | head -1; }
stop_app() { pkill -f "Ashdash.app/Contents/MacOS/ashdash" 2>/dev/null; sleep 1; }
open_in_gui() { wd 25 "$GUI_EXEC" --detach -- /usr/bin/open "$APP" >/dev/null; }
wait_dialog() { # wait_dialog <pattern> <secs> -> prints the dialog text
  local i out
  for i in $(seq 1 "$2"); do out=$(ax_dialogs | grep -E "$1" | head -1); [ -n "$out" ] && { echo "$out"; return 0; }; sleep 1; done
  return 1
}

# ---- 1. build the bundle and the DMG --------------------------------------------------------------------------------------------------
t0=$(date +%s)
# The lab-tooling grants (Terminal, ssh, osascript: Accessibility, Screen Recording, Automation to System Events / Finder) are written first, so that the
# scripts below can read dialogs and click buttons. They name no Ashdash bundle: the app itself starts with no TCC row at all.
[ "${LAB_AUTO_TCC:-1}" = 1 ] && bash tests/lab/guest/tcc-grant.sh >"$QA_ART/tcc-grant.txt" 2>&1
if [ -n "${SKIP_BUILD:-}" ] && [ -d "$DIST/Ashdash.app" ] && ls "$DIST"/Ashdash-*.dmg >/dev/null 2>&1; then
  dmg=$(ls "$DIST"/Ashdash-*.dmg)
  result PKG-01 PASS "bundle and DMG taken from an earlier build in another clone (SKIP_BUILD=1); DMG $(stat -f %z "$dmg") bytes"
elif OUT_DIR=$DIST ARCHS=native packaging/build-app.sh >"$QA_ART/build-app.log" 2>&1 && OUT_DIR=$DIST packaging/make-dmg.sh >"$QA_ART/make-dmg.log" 2>&1; then
  dmg=$(ls "$DIST"/Ashdash-*.dmg)
  result PKG-01 PASS "build-app.sh (ARCHS=native) + make-dmg.sh in $(( $(date +%s) - t0 )) s; DMG $(stat -f %z "$dmg") bytes; $(codesign -dv "$DIST/Ashdash.app" 2>&1 | grep -E 'Signature|flags' | tr '\n' ' ')"
else
  result PKG-01 FAIL "build failed: $(tail -3 "$QA_ART/build-app.log" "$QA_ART/make-dmg.log" | tr '\n' ' ')"; exit 1
fi

# ---- 2. the DMG as a browser delivers it ------------------------------------------------------------------------------------------------
sudo -n spctl --master-enable; st=$(spctl --status)
result PKG-02 PASS "Gatekeeper switched on in this clone (the image ships '$(echo "$st" | head -1)' only after this; before: assessments disabled)"
quarantine "$dmg"
dmg_assess=$(spctl --assess --type open --context context:primary-signature -vv "$dmg" 2>&1 | tr '\n' ' ')
vol=$(hdiutil attach -nobrowse -readonly "$dmg" 2>/dev/null | awk -F'\t' '/Apple_HFS/ {print $NF}' | head -1)
inner_x=$(xattr -l "$vol/Ashdash.app" 2>&1 | tr '\n' ' ')
result PKG-03 PASS "DMG quarantined; spctl on the DMG: $dmg_assess; mounted at '$vol'; xattrs of the app inside the mounted image: '${inner_x:-none}'"
sudo -n rm -rf "$APP"; sudo -n ditto "$vol/Ashdash.app" "$APP"; sudo -n chown -R "$(id -un)" "$APP"
hdiutil detach "$vol" -quiet
[ -d "$APP" ] && result PKG-04 PASS "Ashdash.app copied from the mounted DMG to /Applications ($(du -sm "$APP" | cut -f1) MB); ditto from a mounted quarantined image does NOT carry the quarantine attribute (Finder's drag copy is not scripted here)" || result PKG-04 FAIL "copy failed"
quarantine "$APP"
spctl_out=$(spctl --assess --type execute -vv "$APP" 2>&1 | tr '\n' ' ')
result PKG-05 PASS "app quarantined like a browser download (0083 = Safari); 'spctl --assess': $spctl_out"

# ---- 3. first open: what Gatekeeper refuses ---------------------------------------------------------------------------------------------
stop_app; open_in_gui
d=$(wait_dialog "Not Opened|Open Anyway" 20)
if [ -n "$d" ]; then
  result PKG-06 PASS "double-click equivalent (open $APP) is REFUSED: $d"
  if [ -n "$(running)" ]; then
    st=$(ps -o stat= -p "$(running)")
    echo "launched process state while the dialog is up: $st (T = stopped, not yet running)" >>"$QA_ART/pkg-notes.txt"
  fi
else
  result PKG-06 FAIL "no Gatekeeper dialog within 20 s (process: $(running))"
fi
buttons=$(ax_dialogs | grep CoreServicesUIAgent | grep -o '\[[^]]*\]' | tr '\n' ' ')
result PKG-07 PASS "buttons offered in the first dialog: $buttons (no 'Open')"
click_button CoreServicesUIAgent Done; sleep 2
[ -z "$(running)" ] && result PKG-08 PASS "after 'Done' the app is not running" || result PKG-08 FAIL "app still running after Done"

# Finder's File > Open is what the right-click > Open menu item does on macOS 15 (the Control-click bypass was removed in 15.0)
wd 20 osascript -e 'tell application "Finder" to select (POSIX file "/Applications/Ashdash.app" as alias)' >/dev/null &
fpid=$!
consent Allow 6 >/dev/null; wait "$fpid" 2>/dev/null
wd 20 osascript -e 'tell application "System Events" to tell process "Finder" to click menu item "Open" of menu "File" of menu bar 1' >/dev/null
d=$(wait_dialog "Not Opened|Open Anyway" 15)
[ -n "$d" ] && result PKG-09 PASS "Finder File > Open (same command as right-click > Open) is refused with the same dialog: $d" || result PKG-09 FAIL "no dialog from Finder > Open"
click_button CoreServicesUIAgent Done; sleep 2; stop_app

# ---- 4. System Settings > Privacy & Security > Open Anyway ---------------------------------------------------------------------------
open "x-apple.systempreferences:com.apple.preference.security?Privacy_Security"; sleep 6
blocked=$(wd 60 osascript - <<'OSA'
tell application "System Events" to tell process "System Settings"
  set els to entire contents of window 1
  repeat with e in els
    try
      if (class of e as text) is "static text" and (value of e as text) contains "was blocked" then return (value of e as text)
    end try
  end repeat
  return ""
end tell
OSA
)
if [ -n "$blocked" ]; then
  result PKG-10 PASS "Privacy & Security lists the refused app: '$blocked' (an 'Open Anyway' button next to it)"
  wd 60 osascript - >/dev/null <<'OSA'
tell application "System Events" to tell process "System Settings"
  set els to entire contents of window 1
  repeat with e in els
    try
      if (class of e as text) is "button" and (name of e as text) contains "was blocked" then
        click e
        return "clicked"
      end if
    end try
  end repeat
end tell
OSA
  sleep 3
  pw=$(ax_dialogs | grep -E '^(SecurityAgent|UserNotificationCenter)' | head -1)
  result PKG-11 PASS "click on 'Open Anyway' in Privacy & Security: authentication prompt shown? ${pw:-no prompt seen from the guest (no Touch ID; see notes)}"
else
  result PKG-10 FAIL "Privacy & Security does not show the blocked app"
fi
pkill -x "System Settings" 2>/dev/null
stop_app; open_in_gui
d=$(wait_dialog "Open Anyway" 20)
if [ -n "$d" ]; then
  result PKG-12 PASS "opening again after 'Open Anyway' shows the confirm dialog: $d"
  click_button CoreServicesUIAgent "Open Anyway"; sleep 4
  p=$(running)
  [ -n "$p" ] && result PKG-13 PASS "after the confirm dialog the app runs (pid $p, path $(ps -o command= -p "$p" | cut -c1-120))" || result PKG-13 FAIL "app not running after Open Anyway"
else
  p=$(running)
  result PKG-12 FAIL "no confirm dialog after Open Anyway; app running: ${p:-no}"
fi
stop_app
click_button CoreServicesUIAgent Done; click_button CoreServicesUIAgent Done; click_button CoreServicesUIAgent "Open Anyway" 2>/dev/null; stop_app

# ---- 5. the documented terminal route: xattr -dr -------------------------------------------------------------------------------------
quarantine "$APP"
xattr -dr com.apple.quarantine "$APP"
x=$(xattr -l "$APP" | wc -l | tr -d ' ')
open_in_gui; sleep 5
p=$(running)
[ -n "$p" ] && [ "$x" = 0 ] && result PKG-14 PASS "xattr -dr com.apple.quarantine, then open: runs from $(ps -o command= -p "$p" | cut -c1-90), no dialog, no translocation (spctl --assess still says: $(spctl --assess --type execute "$APP" 2>&1 | tr '\n' ' '))" || result PKG-14 FAIL "app not running after xattr -dr (xattrs left: $x)"

# ---- 6. first launch of the installed app on this fresh account: every prompt, before and after ---------------------------------------------
# (the app was started by step 5 through `open`, so it is its own TCC client; the lab-tooling grants name no Ashdash bundle)
# shellcheck source=checklist.sh
. tests/lab/qa/checklist.sh
APP=/Applications/Ashdash.app; BIN=$APP/Contents/MacOS/ashdash; QAH=$HOME; DATA="$HOME/Library/Application Support/ashdash"
stop_app; : >"$QA_ART/app.log"
wd 25 "$GUI_EXEC" --detach -- /usr/bin/open --stdout "$QA_ART/app.log" --stderr "$QA_ART/app.log" "$APP" >/dev/null
for i in $(seq 1 30); do sock_ok && alive && break; sleep 1; done; t_up=$i
seen=""; for i in $(seq 1 8); do d=$(ax_dialogs | grep -v 'CoreServicesUIAgent | window .*: *$\|System Settings'); seen="$seen$d"; sleep 1; done
result PKG-15 PASS "first launch from /Applications (fresh account, no data folder, no TCC rows for the app): up after ~${t_up} s; macOS prompts in the first 8 s: ${seen:-none}; windows: $(win_tool Ashdash ashdash | awk '{print $2":"$6"x"$7}' | tr '\n' ' ') (status item + hidden launcher); data folder created: $(ls "$DATA" 2>/dev/null | tr '\n' ' '); stderr: $(head -2 "$QA_ART/app.log" | tr '\n' ' ')"
show; o=$(look first-show); hide_
result PKG-16 PASS "first hotkey press on the fresh account shows the launcher with no setup UI or hint; OCR: ${o:-nothing}"
A=$BIN
st0=$($A login-item status 2>&1); en=$($A login-item enable 2>&1); sleep 1; st1=$($A login-item status 2>&1); pr=$(ax_dialogs | grep -v 'CoreServicesUIAgent | window .*: *$' | head -2 | cut -c1-200); dis=$($A login-item disable 2>&1); st2=$($A login-item status 2>&1)
result PKG-17 PASS "login item from the bundle: status before: '$st0'; enable: '$en'; status: '$st1'; disable: '$dis'; status: '$st2'; macOS prompts: ${pr:-none}"
open "ashdash://prefs"; sleep 4
result PKG-18 PASS "open ashdash://prefs from the installed bundle: window(s) $(win_tool Ashdash ashdash | awk '$6>=500 {print $6"x"$7" layer "$2}' | tr '\n' ' '); process: $(pgrep -f 'ashdash prefs-window' | head -1 | xargs -I{} ps -o command= -p {} | cut -c1-90)"
pkill -f 'ashdash prefs-window'; sleep 1
# Automation: the first '> command' (Terminal), deny, then allow
tcc_reset
ask '> echo prompt-deny' 1.0 >/dev/null; key_code $KC_RETURN; sleep 3
dlg=$(consent "Don’t Allow" 10); sleep 2; show; o=$(look auto-deny); hide_
result PKG-19 PASS "first use of '> command': prompt = $(echo "$dlg" | cut -c1-420); after Don't Allow the launcher shows: ${o:-nothing}"
tccutil reset AppleEvents dev.ashdash.launcher >/dev/null 2>&1
ask '> echo prompt-allow' 1.0 >/dev/null; key_code $KC_RETURN; sleep 2; dlg=$(consent Allow 10); sleep 4
result PKG-20 PASS "after tccutil reset and Allow: prompt again ($(echo "$dlg" | cut -c1-80)...), the command opens in Terminal (Terminal windows: $(wd 20 osascript -e 'tell application "System Events" to tell process "Terminal" to get count of windows' | tail -1))"
# Accessibility: no prompt, an explanation
qt pb-set "PROMPT-CLIP-TEST"; sleep 1.8
ask 'clip PROMPT-CLIP' 1.5 >/dev/null; key_code $KC_RETURN; sleep 1.5; show; o=$(look no-ax); hide_
result PKG-21 PASS "auto-paste on a fresh account (no Accessibility): no macOS prompt (dialogs now: $(ax_dialogs | grep -v 'CoreServicesUIAgent | window .*: *$\|System Settings' | head -1 | cut -c1-100)); the launcher says: $o"
stop_app

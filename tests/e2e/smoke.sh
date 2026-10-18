#!/usr/bin/env bash
# Smoke test for Ashdash, run by the e2e workflow on a GitHub-hosted macOS runner (tests/e2e/ci/run-smoke.sh; cwd = the checkout):
#
#   bash tests/e2e/ci/run-smoke.sh          (CI only: it drives the real keyboard, screen and hotkey; docs/TESTING.md)
#
# Stages (each records PASS / FAIL / SKIP plus the reason in $E2E_ARTIFACTS/smoke-report.txt, which is also
# printed at the end):  env, build (debug ashdash), check.sh (unless --quick), bundle (native .app so the
# ashdash:// scheme can register), gui (launch in the GUI session with a temp HOME, `ashdash show`,
# `ashdash search saf`, `open ashdash://search?query=saf`, the global hotkey via System Events, window
# state read back as TEXT, login-item enable/status/disable, quit).
# Screenshots are artifacts only (never committed). Steps needing the manual TCC grants are SKIPPED with
# the exact reason when the runner lacks them. Exit status: 1 when a build/check/bundle stage FAILs
# or a GUI stage FAILs; SKIPs do not fail the job.
# shellcheck disable=SC2015,SC2329  # "A && rec || rec" is intentional; cleanup runs from the EXIT trap
set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1
ROOT=$PWD
quick=0 gui=1
for a in "$@"; do case "$a" in --quick) quick=1 ;; --no-gui) gui=0 ;; *) echo "smoke: unknown option $a" >&2; exit 2 ;; esac; done

ART=${E2E_ARTIFACTS:-$(mktemp -d "${TMPDIR:-/tmp}/smoke-art.XXXXXX")}
mkdir -p "$ART"
REPORT=$ART/smoke-report.txt
: >"$REPORT"
export PATH="$HOME/.cargo/bin:$PATH"
failed=0
rec() { # rec PASS|FAIL|SKIP <stage> <detail>
  printf '%-4s %-22s %s\n' "$1" "$2" "$3" | tee -a "$REPORT"
  [ "$1" = FAIL ] && failed=1
  return 0
}
WORK=$(mktemp -d "${TMPDIR:-/tmp}/smoke-work.XXXXXX")
GUI_EXEC=$ROOT/tests/e2e/guest/gui-exec
cleanup() {
  if [ -n "${BIN:-}" ]; then HOME=$WORK/home "$BIN" quit >/dev/null 2>&1 || true; fi
  pkill -f "$WORK" 2>/dev/null || true
  [ -n "${DIST:-}" ] && pkill -f "$DIST" 2>/dev/null || true
  rm -rf "$WORK" "${DIST:-/nonexistent}"
}
trap cleanup EXIT

# ---- env -------------------------------------------------------------------------------------
{
  echo "date:       $(date)"
  echo "host:       $(uname -a)"
  echo "macOS:      $(sw_vers -productVersion) ($(sw_vers -buildVersion))"
  echo "sip:        $(csrutil status 2>&1 | head -1)"
  echo "rustc:      $(rustc --version 2>&1)"
  echo "revision:   $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo 'n/a (no .git in the VM)')"
  echo "console:    $(stat -f %Su /dev/console)"
} >"$ART/env.txt"
rec PASS env "$(sw_vers -productVersion) $(uname -m), $(rustc --version 2>&1 | cut -d' ' -f1-2)"

# ---- build / check ---------------------------------------------------------------------------
t0=$(date +%s)
if cargo build -p ashdash >"$ART/build.log" 2>&1; then
  rec PASS build "cargo build -p ashdash in $(( $(date +%s) - t0 )) s"
else
  rec FAIL build "cargo build failed, tail: $(tail -3 "$ART/build.log" | tr '\n' ' ')"
  cat "$REPORT"; exit 1
fi
BIN=$ROOT/target/debug/ashdash
if [ "$quick" = 1 ]; then
  rec SKIP check.sh "--quick"
else
  t0=$(date +%s)
  if scripts/check.sh >"$ART/check.log" 2>&1; then
    rec PASS check.sh "check.sh: OK in $(( $(date +%s) - t0 )) s"
  else
    rec FAIL check.sh "failed, tail: $(tail -5 "$ART/check.log" | tr '\n' ' ')"
  fi
fi
if [ "$gui" = 0 ]; then rec SKIP gui "--no-gui"; cat "$REPORT"; exit "$failed"; fi

# ---- headless CLI checks that need no session ---------------------------------------------------
mkdir -p "$WORK/home"
export HOME_REAL=$HOME
if HOME=$WORK/home "$BIN" query saf >"$ART/query-saf.txt" 2>&1 && grep -qi safari "$ART/query-saf.txt"; then
  rec PASS cli-query "'ashdash query saf' lists Safari"
else
  rec FAIL cli-query "'ashdash query saf' did not list Safari (see query-saf.txt)"
fi

# ---- native bundle (needed for the URL scheme + login item) --------------------------------------
APP=""
t0=$(date +%s)
# LaunchServices ignores apps under /var/folders ($TMPDIR): "No application knows how to open URL ashdash://..." (-10814).
# The bundle therefore lives in the home directory of the (throw-away) runner user.
DIST=$HOME/e2e-smoke-dist
if ARCHS=native SKIP_ICON=1 OUT_DIR=$DIST packaging/build-app.sh >"$ART/bundle.log" 2>&1; then
  APP=$DIST/Ashdash.app
  rec PASS bundle "native Ashdash.app built and verified in $(( $(date +%s) - t0 )) s"
  spctl --assess --type execute "$APP" >"$ART/spctl.txt" 2>&1
  rec PASS gatekeeper "informational: spctl says: $(tr '\n' ' ' <"$ART/spctl.txt")"
else
  rec FAIL bundle "build-app.sh failed, tail: $(tail -3 "$ART/bundle.log" | tr '\n' ' ')"
fi
RUN_BIN=${APP:+$APP/Contents/MacOS/ashdash}
RUN_BIN=${RUN_BIN:-$BIN}

# ---- GUI session available? ---------------------------------------------------------------------
if [ "$(stat -f %Su /dev/console)" != "$(id -un)" ]; then
  rec SKIP gui "nobody is logged in on the console as $(id -un) (auto-login missing): no GUI session"
  cat "$REPORT"; exit "$failed"
fi
HOME_G=$WORK/home
launch_out=$(HOME=$HOME_G "$GUI_EXEC" --detach -- "$RUN_BIN" 2>&1)
echo "$launch_out" >"$ART/launch.txt"
up=""
for _ in $(seq 1 30); do
  if HOME=$HOME_G "$BIN" paths >/dev/null 2>&1 && pgrep -f "$RUN_BIN" >/dev/null && { [ -S "$HOME_G/Library/Application Support/ashdash/run/ipc.sock" ] || [ -S "/tmp/ashdash-$(id -u)/ipc.sock" ]; }; then up=1; break; fi
  sleep 1
done
if [ -z "$up" ]; then
  rec FAIL gui-launch "launcher did not start in the GUI session (job: $launch_out)"
  cat "$REPORT"; exit 1
fi
rec PASS gui-launch "launcher running in the GUI session (pid $(pgrep -f "$RUN_BIN" | head -1))"

# CLI -> single-instance socket
if HOME=$HOME_G "$BIN" show >"$ART/show.txt" 2>&1; then rec PASS cli-show "ashdash show answered: $(head -c 120 "$ART/show.txt" | tr '\n' ' ')"; else rec FAIL cli-show "ashdash show failed: $(head -c 200 "$ART/show.txt")"; fi
if HOME=$HOME_G "$BIN" search saf >"$ART/search.txt" 2>&1; then rec PASS cli-search "ashdash search saf answered"; else rec FAIL cli-search "ashdash search saf failed: $(head -c 200 "$ART/search.txt")"; fi

# ashdash:// URL scheme (registered by LaunchServices from the bundle)
if [ -n "$APP" ]; then
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP" >/dev/null 2>&1
  if open "ashdash://search?query=saf" >"$ART/url.txt" 2>&1; then
    sleep 2
    rec PASS url-scheme "open ashdash://search?query=saf returned 0 (handler registered); launcher alive: $(pgrep -f "$RUN_BIN" >/dev/null && echo yes || echo NO)"
  else
    rec FAIL url-scheme "open ashdash://search?query=saf failed: $(head -c 200 "$ART/url.txt")"
  fi
else
  rec SKIP url-scheme "no bundle was built"
fi

# Global hotkey + window state through System Events (needs Accessibility for Terminal; tests/e2e/guest/tcc-grant.sh grants it on a runner)
se_probe() { "$GUI_EXEC" --timeout 20 -- osascript -e 'tell application "System Events" to get name of first process whose frontmost is true' 2>&1; }
# A missing Automation grant makes osascript wait for a consent dialog nobody clicks; gui-exec then reports "timed out".
se_denied() { echo "$1" | grep -Eq '1002|not allowed|assistive|not authorized|-1743|timed out'; }
# The runner image has SIP off, so the grants can be written into this throw-away machine. Do it BEFORE the
# first System Events call: a call that raised a consent dialog first leaves tccd/Terminal in a state that the grant does not clear.
granted=0
if [ "${E2E_AUTO_TCC:-1}" = 1 ] && csrutil status 2>&1 | grep -q disabled; then
  bash "$ROOT/tests/e2e/guest/tcc-grant.sh" >"$ART/tcc-grant.txt" 2>&1 && granted=1
fi
SE_PROBE=$(se_probe)
if [ "$granted" = 1 ] && ! se_denied "$SE_PROBE"; then
  rec PASS tcc-grant "tcc-grant.sh (SIP is off) granted Terminal/sshd in this runner only (E2E_AUTO_TCC=0 disables)"
fi
if se_denied "$SE_PROBE"; then
  rec SKIP hotkey "System Events denied (Accessibility/Automation not granted to Terminal on this runner): $(echo "$SE_PROBE" | head -c 160)"
  rec SKIP window-state "same reason"
else
  rec PASS system-events "System Events works; frontmost app before: $SE_PROBE"
  # The hotkey TOGGLES the launcher, and the url-scheme stage above may have left it visible: note the state, press, and require
  # that the window count changed; if that hid it, press again and require it to be back.
  # These steps run straight from the ssh session (its System Events / screencapture grants are in the clone), NOT through gui-exec:
  # gui-exec activates Terminal, and a launcher that loses focus hides itself, which made the window vanish between press and check.
  wd() { perl -e 'alarm shift; exec @ARGV' 20 "$@" 2>&1; }   # a missing grant blocks on a consent dialog: never wait forever
  nwin() { wd osascript -e 'tell application "System Events" to tell process "ashdash" to get count of windows' | tail -1; }
  press() { wd osascript -e 'tell application "System Events" to key code 49 using {option down, shift down}' >>"$ART/hotkey.txt"; sleep 2; }
  w0=$(nwin); press; w1=$(nwin); w2=$w1
  if [ "$w1" = 0 ]; then press; w2=$(nwin); fi
  wd osascript -e 'tell application "System Events" to keystroke "saf"' >>"$ART/hotkey.txt"
  sleep 1
  wd screencapture -x "$ART/hotkey-saf.png" >>"$ART/hotkey.txt"
  wins=$(wd osascript -e 'tell application "System Events" to tell process "ashdash" to get {name, position, size} of every window')
  echo "windows before / after first press / after second: $w0 / $w1 / $w2; ashdash windows (name,x,y,w,h): $wins" >"$ART/window-state.txt"
  if [ "$w0" != "$w1" ] && [ "$w2" -ge 1 ] 2>/dev/null; then
    rec PASS hotkey "Option+Shift+Space toggles the launcher (windows $w0 -> $w1 -> $w2); window (name,x,y,w,h)=$wins"
  else
    rec FAIL hotkey "hotkey did not toggle the launcher window (windows $w0 -> $w1 -> $w2; see window-state.txt, hotkey-saf.png)"
  fi
  [ -s "$ART/hotkey-saf.png" ] && rec PASS screenshot "hotkey-saf.png $(stat -f %z "$ART/hotkey-saf.png") bytes (artifact only; may show only the wallpaper without the Screen Recording grant)" || rec SKIP screenshot "screencapture produced no file: $(tail -1 "$ART/hotkey.txt")"
fi

# login item (registers inside the VM only)
if [ -n "$APP" ]; then
  out=$(HOME=$HOME_G "$APP/Contents/MacOS/ashdash" login-item enable 2>&1; HOME=$HOME_G "$APP/Contents/MacOS/ashdash" login-item status 2>&1; HOME=$HOME_G "$APP/Contents/MacOS/ashdash" login-item disable 2>&1)
  echo "$out" >"$ART/login-item.txt"
  rec PASS login-item "enable/status/disable ran: $(echo "$out" | tr '\n' ' ' | head -c 200)"
else
  rec SKIP login-item "no bundle was built"
fi

# quit
HOME=$HOME_G "$BIN" quit >/dev/null 2>&1; sleep 2
if pgrep -f "$RUN_BIN" >/dev/null; then rec FAIL quit "launcher still running after 'ashdash quit'"; else rec PASS quit "launcher exited on 'ashdash quit'"; fi

echo "---- report ----"; cat "$REPORT"
exit "$failed"

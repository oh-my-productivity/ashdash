#!/usr/bin/env bash
# Screenshots of the real launcher window in its two plain states, taken on a GitHub-hosted macOS runner (CI only: it shows a window and
# captures the screen, so it never runs on a developer's Mac; docs/TESTING.md):
#
#   bash tests/e2e/ci/run-launcher-look.sh        (the `launcher-look` job of e2e.yml uploads $E2E_ARTIFACTS/shots as an artifact)
#
# What it does (cwd = the checkout):
#   1. builds the debug binary and grants TCC in this throw-away runner (tests/e2e/guest/tcc-grant.sh; E2E_AUTO_TCC=0 skips),
#   2. starts the launcher in the GUI session with an empty temporary HOME (the default look: no theme is chosen),
#   3. `ashdash show` -> the EMPTY state; several captures a third of a second apart (the caret blinks, so one of them has it),
#      once with and once without the window shadow; the window's size and position are written to window.txt,
#   4. `ashdash search saf` -> the state WITH results (the panel grows downward); the same captures.
#   4a. a query nothing matches (`zzqxvk`) -> `fbonly`: only the web-search fallbacks, the first selected (its URL in mono), the others
#      without URL. (The hairline above the first fallback is in `results`. Sending the Down arrow from a GUI-session job made the
#      launcher lose focus and hide, so selecting a later fallback is not captured.)
#   4b. a path query into a deep folder made under the temporary HOME -> `longpath`: rows whose path subtitle is far wider than the text
#      column, so the middle elision (`~/…/Final Drafts/file`) and the text stopping before the hint column can be seen.
#   5. edge-check.py reads the captures: the outermost pixel ring of every no-shadow capture, and the ring just outside the border in the
#      with-shadow capture, must be the border colour (or transparent), never light (luminance <= EDGE_MAX_LUM, default 80). A light
#      edge pixel fails the suite: that is the "white border" of macOS 26 (docs/design/launcher-look.md) and it must not come back.
# Output: $E2E_ARTIFACTS/shots/*.png, window.txt, edge-check.txt, log.txt. The images are workflow artifacts (7 days), never committed.
# Apart from the edge check nothing compares them with a reference image: a person looks at them (docs/design/launcher-look.md has the
# numbers to look for).
# shellcheck disable=SC2015
set -uo pipefail
cd "$(dirname "$0")/../../.." || exit 1
ROOT=$PWD
ART=${E2E_ARTIFACTS:-$(mktemp -d "${TMPDIR:-/tmp}/launcher-look.XXXXXX")}
SHOTS=$ART/shots
mkdir -p "$SHOTS"
export PATH="$HOME/.cargo/bin:$PATH"
GUI_EXEC=$ROOT/tests/e2e/guest/gui-exec
log() { echo "[look] $*" | tee -a "$ART/log.txt"; }

cargo build -p ashdash >"$ART/build.log" 2>&1 || { tail -20 "$ART/build.log"; exit 1; }
BIN=$ROOT/target/debug/ashdash
if [ "${E2E_AUTO_TCC:-1}" = 1 ] && csrutil status 2>&1 | grep -q disabled; then
  bash "$ROOT/tests/e2e/guest/tcc-grant.sh" >"$ART/tcc-grant.txt" 2>&1 && log "tcc-grant ok" || log "tcc-grant FAILED (see tcc-grant.txt)"
fi

WORK=$(mktemp -d "${TMPDIR:-/tmp}/launcher-look-work.XXXXXX")
export LOOK_HOME=$WORK/home
mkdir -p "$LOOK_HOME"
cleanup() { HOME=$LOOK_HOME "$BIN" quit >/dev/null 2>&1; pkill -f "$BIN" 2>/dev/null; rm -rf "$WORK"; }
trap cleanup EXIT

# "id x y w h" of the launcher window: the biggest window of the process at the panel layer (25); the status item is 32 wide
launcher_window() {
  osascript -l JavaScript -e '
    ObjC.import("CoreGraphics");
    var l = ObjC.deepUnwrap(ObjC.castRefToObject($.CGWindowListCopyWindowInfo(1, 0)));
    var w = l.filter(function(w){ return /^ashdash$/i.test(w.kCGWindowOwnerName) && w.kCGWindowLayer == 25 && w.kCGWindowBounds.Width > 100; })
             .map(function(w){ var b = w.kCGWindowBounds; return [w.kCGWindowNumber, b.X, b.Y, b.Width, b.Height].join(" "); });
    w.length ? w[0] : ""' 2>/dev/null
}
capture() { # capture <state>: six window-only captures (no shadow), one with the shadow, one full screen
  local state=$1 id i
  read -r id _ < <(launcher_window)
  if [ -z "${id:-}" ]; then log "no launcher window in state $state"; return 1; fi
  echo "$state: $(launcher_window)  (id x y w h, points)" >>"$ART/window.txt"
  for i in 1 2 3 4 5 6; do
    screencapture -l "$id" -o -x "$SHOTS/$state-$i.png" 2>>"$ART/log.txt"; sleep 0.33
  done
  screencapture -l "$id" -x "$SHOTS/$state-with-shadow.png" 2>>"$ART/log.txt"
  screencapture -x "$SHOTS/$state-screen.png" 2>>"$ART/log.txt"
  n=0
  for f in "$SHOTS/$state-"*; do [ -e "$f" ] && n=$((n + 1)); done
  log "$state: $n images"
}

# a deep folder with long names: its files have path subtitles far wider than the text column of a result row
LONG_REL="Documents/Projects/Customer Work/2026/Quarterly Reports/Final Drafts"
mkdir -p "$LOOK_HOME/$LONG_REL"
: >"$LOOK_HOME/$LONG_REL/Annual-Summary-Draft-v3.txt"
: >"$LOOK_HOME/$LONG_REL/Meeting notes with the regional finance leads and the audit committee.md"
: >"$LOOK_HOME/$LONG_REL/short.txt"

HOME=$LOOK_HOME "$GUI_EXEC" --detach -- "$BIN" >>"$ART/log.txt" 2>&1
up=""
for _ in $(seq 1 40); do
  if HOME=$LOOK_HOME "$BIN" paths >/dev/null 2>&1 && pgrep -f "$BIN" >/dev/null; then up=1; break; fi
  sleep 1
done
[ -n "$up" ] || { log "the launcher did not start"; exit 1; }
sleep 3

rc=0
HOME=$LOOK_HOME "$BIN" show >>"$ART/log.txt" 2>&1; sleep 2
capture empty || rc=1
HOME=$LOOK_HOME "$BIN" search saf >>"$ART/log.txt" 2>&1; sleep 2.5
capture results || rc=1
# a query nothing matches: only the web-search fallbacks remain, the first one selected (its URL shown in mono), the others without URL
HOME=$LOOK_HOME "$BIN" search zzqxvk >>"$ART/log.txt" 2>&1; sleep 2.5
capture fbonly || rc=1
# the literal "~" is the query text, ashdash expands it itself
# shellcheck disable=SC2088
HOME=$LOOK_HOME "$BIN" search "~/$LONG_REL/" >>"$ART/log.txt" 2>&1; sleep 2.5
capture longpath || rc=1
cat "$ART/window.txt" 2>/dev/null

# the pixel check of the edges (a light pixel on the window edge fails the suite)
log "display: $(system_profiler SPDisplaysDataType 2>/dev/null | grep -E 'Resolution|UI Looks like|Display Type' | tr -s ' ' | tr '\n' ';')"
python3 "$ROOT/tests/e2e/screenshots/edge-check.py" --selftest >>"$ART/log.txt" 2>&1 || { log "edge-check selftest FAILED"; rc=1; }
python3 "$ROOT/tests/e2e/screenshots/edge-check.py" --max-lum "${EDGE_MAX_LUM:-80}" --report "$ART/edge-check.txt" "$SHOTS" >/dev/null 2>>"$ART/log.txt" \
  && log "edge-check: ok" || { log "edge-check: FAILED (see edge-check.txt)"; rc=1; }
grep -E "FAIL|^edge-check" "$ART/edge-check.txt" 2>/dev/null
exit "$rc"

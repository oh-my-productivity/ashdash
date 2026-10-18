#!/usr/bin/env bash
# GUI smoke test: launch the debug binary with a throwaway HOME, press the global hotkey
# (Option+Shift+Space), type a query, take a screenshot, print its path, then clean up.
#
# WARNING: this drives the real keyboard and screen of the machine it runs on, so it runs on CI only (it refuses elsewhere; scripts/ci-run.sh).
#   * It takes over the global hotkey: stop any other Ashdash instance first (two instances fight).
#   * The screenshot captures the WHOLE desktop, including other windows. Look at it, then delete it.
#     Never commit it.
#   * Needs Accessibility permission for the terminal running it (System Events keystrokes).
#   * The app's clipboard watcher runs against the real pasteboard; its history lands in the
#     throwaway HOME and is deleted on exit.
#
# Usage:   QA_GUI_OK=1 scripts/qa/gui-smoke.sh [query]        (default query: saf)
# Env:     QA_GUI_OK=1        required acknowledgement of the warning above
#          QA_NO_BUILD=1      skip `cargo build` (use the existing target/debug/ashdash)
#          QA_STARTUP_WAIT=5  seconds to let the app start (default 5)
#          QA_BIN=path        binary to run (default target/debug/ashdash)
# Output:  last line printed to stdout is the screenshot path. Exit status is non-zero on any failure.
set -euo pipefail
# shellcheck source=require-ci.sh
. "$(dirname "${BASH_SOURCE[0]}")/require-ci.sh"

QUERY="${1:-saf}"
STARTUP_WAIT="${QA_STARTUP_WAIT:-5}"

if [[ "${QA_GUI_OK:-}" != "1" ]]; then
    echo "gui-smoke: refusing to run: this takes over the keyboard, screen and global hotkey." >&2
    echo "gui-smoke: re-run with QA_GUI_OK=1 once nothing else is using them." >&2
    exit 2
fi
if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "gui-smoke: macOS only" >&2
    exit 2
fi
# Never send keystrokes with a query that could be interpreted by AppleScript quoting.
if [[ ! "$QUERY" =~ ^[A-Za-z0-9\ ._-]+$ ]]; then
    echo "gui-smoke: query may only contain letters, digits, space, '.', '_' and '-'" >&2
    exit 2
fi

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if pgrep -x ashdash >/dev/null 2>&1; then
    echo "gui-smoke: another 'ashdash' process is running; quit it first (the hotkey would be contested)" >&2
    exit 2
fi

# Build with the real HOME (cargo/rustup live there), before HOME is replaced.
if [[ "${QA_NO_BUILD:-}" != "1" ]]; then
    cargo build -q -p ashdash
fi
BIN="${QA_BIN:-$ROOT/target/debug/ashdash}"
[[ -x "$BIN" ]] || { echo "gui-smoke: binary not found: $BIN" >&2; exit 1; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ashdash-smoke.XXXXXX")"
SHOT="$(mktemp "${TMPDIR:-/tmp}/ashdash-smoke-shot.XXXXXX")"
mv "$SHOT" "$SHOT.png"
SHOT="$SHOT.png"
PID=""

cleanup() {
    local rc=$?
    if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; then
        kill "$PID" 2>/dev/null || true
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            kill -0 "$PID" 2>/dev/null || break
            sleep 0.2
        done
        kill -9 "$PID" 2>/dev/null || true
    fi
    if [[ $rc -ne 0 && -f "$WORK/app.log" ]]; then
        echo "gui-smoke: app log tail:" >&2
        tail -n 20 "$WORK/app.log" >&2 || true
    fi
    rm -rf "$WORK"
    if [[ $rc -ne 0 ]]; then
        rm -f "$SHOT"
    fi
    exit $rc
}
trap cleanup EXIT INT TERM

mkdir -p "$WORK/home"
export HOME="$WORK/home"
unset XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME

"$BIN" >"$WORK/app.log" 2>&1 &
PID=$!

# Let the app start; fail early if it dies.
for _ in $(seq 1 $((STARTUP_WAIT * 5))); do
    kill -0 "$PID" 2>/dev/null || { echo "gui-smoke: app exited during startup" >&2; exit 1; }
    sleep 0.2
done

osa() { osascript -e "$1" >/dev/null; }

# key code 49 = Space. The window hides on focus loss, so keep the waits short.
osa 'tell application "System Events" to key code 49 using {option down, shift down}'
sleep 1
osa "tell application \"System Events\" to keystroke \"$QUERY\""
sleep 1

screencapture -x "$SHOT"
[[ -s "$SHOT" ]] || { echo "gui-smoke: screencapture produced no file (Screen Recording permission?)" >&2; exit 1; }

kill -0 "$PID" 2>/dev/null || { echo "gui-smoke: app died during the test" >&2; exit 1; }

echo "gui-smoke: OK (app alive after hotkey + typing '$QUERY')" >&2
echo "gui-smoke: screenshot shows the whole desktop: view it, then delete it" >&2
echo "$SHOT"

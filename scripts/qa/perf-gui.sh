#!/usr/bin/env bash
# GUI-process performance measurement of the real release binary (cold start, idle RSS/CPU,
# memory after use, hotkey latency). See docs/PERF.md section "GUI process" for what it measures
# and what is only a proxy.
#
# It SHOWS WINDOWS and posts synthetic key events on the real desktop. It therefore holds the shared
# desktop lock (mkdir /tmp/ashdash-desktop.lock; polls up to 10 min, always released on exit),
# refuses to run while any `ashdash` process exists, uses a temporary HOME (the user's data folder
# is never read or written) and kills every ashdash process it started.
# Needs Accessibility permission for the terminal (perf-probe posts CGEvents) and swiftc.
#
#   scripts/qa/perf-gui.sh [path-to-release-ashdash]
# Env: PERF_VARIANTS=empty,real,synth5000  PERF_REPS=3  PERF_IDLE_S=60  PERF_CYCLES=20  PERF_LAT_N=10
#      PERF_REP_START=1 (first repetition number; the desktop lock is held for one invocation, keep it under ~5 min: one variant, one rep)
#      PERF_MODE=latency|mem|gap (latency: hotkey and `ashdash show` -> window visible only; mem: footprint with the window shown only;
#        each about 40 s per repetition; gap: latency vs. time hidden)
#        typing: key-to-glyph and key-to-results latency of typing PERF_TYPE_TEXT (default `pup`) key by key, on the busy-laptop
#        variants PERF_VARIANTS=heavy,heavy-slow (PERF_TYPE_GAP_MS=110 PERF_TYPE_RUNS=15 PERF_SLOW_FILTER_S=0.25)
#      PERF_OUT=<dir for results.jsonl and the summary>  (default: a new dir under /tmp)
set -uo pipefail
# shellcheck source=require-ci.sh
. "$(dirname "${BASH_SOURCE[0]}")/require-ci.sh"

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${1:-$ROOT/target/release/ashdash}"
LOCK=/tmp/ashdash-desktop.lock
HAVE_LOCK=0
T="$(mktemp -d /tmp/alfperf.XXXXXX)"

cleanup() {
    pkill -x ashdash 2>/dev/null          # only ours can exist: we refuse to start otherwise
    sleep 0.5
    pkill -9 -x ashdash 2>/dev/null
    [ "$HAVE_LOCK" = 1 ] && rmdir "$LOCK" 2>/dev/null
    rm -rf "$T/h-"* "$T/apps5000" "$T/probe-build"
    echo "perf-gui: results in ${PERF_OUT:-$T/out} (temp dir $T)"
}
trap cleanup EXIT INT TERM

[ -x "$BIN" ] || { echo "perf-gui: binary not found: $BIN (cargo build --release)" >&2; exit 2; }
for _ in $(seq 1 120); do
    if mkdir "$LOCK" 2>/dev/null; then HAVE_LOCK=1; break; fi
    sleep 5
done
[ "$HAVE_LOCK" = 1 ] || { echo "perf-gui: could not get $LOCK" >&2; exit 2; }
if pgrep -x ashdash >/dev/null; then echo "perf-gui: an ashdash is already running" >&2; exit 2; fi

swiftc -O "$ROOT/scripts/qa/perf-probe.swift" -o "$T/perf-probe" 2>"$T/swiftc.log" || { cat "$T/swiftc.log" >&2; exit 2; }
[ "$("$T/perf-probe" trusted)" = "true" ] || { echo "perf-gui: this terminal has no Accessibility permission (synthetic key events would be dropped)" >&2; exit 2; }

if [[ ",${PERF_VARIANTS:-empty,real,synth5000}," == *",synth5000,"* || ",${PERF_VARIANTS:-}," == *",heavy,"* || ",${PERF_VARIANTS:-}," == *",heavy-slow,"* ]]; then
    python3 - "$T/apps5000" <<'PY'
import os, random, sys
d = sys.argv[1]; os.makedirs(d)
words = ("Studio Mail Pad Code Photo Video Music Note Calendar Chat Draw Sketch Terminal Browser Player Editor Manager Helper Hub "
         "Cloud Sync Backup Cleaner Monitor Reader Writer Design Build Deploy Server Client Vault Key Pass Map Weather Clock Timer").split()
random.seed(7)
INFO = ('<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
        '<plist version="1.0"><dict><key>CFBundleName</key><string>X</string><key>CFBundleIdentifier</key><string>com.example.x</string>'
        '<key>CFBundleIconFile</key><string>icon</string></dict></plist>')
names = ["Safari", "Visual Studio Code"] + [" ".join(random.choice(words) for _ in range(random.randint(1, 3))) + f" {i}" for i in range(5000)]
# The busy-laptop variants (typing lag) give every app a real icon file, as a laptop's apps have: the launcher then converts it with
# `sips` and loads the PNG for each new result row, which is work that happens on every keystroke. All apps share one file (hard links).
icon = None
if "heavy" in os.environ.get("PERF_VARIANTS", ""):
    import struct, zlib
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    raw = b"".join(b"\0" + bytes(((x * 2 + y) % 256, (x + y * 2) % 256, (x * y) % 256, 255)[i] for x in range(128) for i in range(4)) for y in range(128))
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 128, 128, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
    icon = d + ".icon.icns"
    with open(icon, "wb") as f:
        f.write(b"icns" + struct.pack(">I", 16 + len(png)) + b"ic07" + struct.pack(">I", 8 + len(png)) + png)
for n in names:
    os.makedirs(f"{d}/{n}.app/Contents")
    open(f"{d}/{n}.app/Contents/Info.plist", "w").write(INFO)
    if icon:
        os.makedirs(f"{d}/{n}.app/Contents/Resources")
        os.link(icon, f"{d}/{n}.app/Contents/Resources/icon.icns")
PY
fi

export PERF_BIN="$BIN" PERF_PROBE="$T/perf-probe" PERF_ROOT="$T" PERF_ROOT_REPO="$ROOT"
export PERF_OUT="${PERF_OUT:-$T/out}"
mkdir -p "$PERF_OUT"
echo "perf-gui: start $(date) uptime: $(uptime)"
python3 "$ROOT/scripts/qa/perf-gui.py"
rc=$?
echo "perf-gui: end $(date) uptime: $(uptime)"
exit $rc

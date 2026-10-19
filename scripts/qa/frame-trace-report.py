#!/usr/bin/env python3
"""Read the launcher's frame trace and say where a slow keystroke went. For the owner's own machine, no probe or CI needed:

    killall ashdash; ASHDASH_FRAME_TRACE=1 /Applications/Ashdash.app/Contents/MacOS/ashdash 2> /tmp/ashdash-ft.txt &
    # press the hotkey, type `pup` the way it feels slow, press Esc, then:
    python3 scripts/qa/frame-trace-report.py /tmp/ashdash-ft.txt

Paste the output into the issue. (From source: `ASHDASH_FRAME_TRACE=1 cargo run --release -p ashdash 2> /tmp/ashdash-ft.txt`.)
The trace itself is documented in crates/app/src/gui/frametrace.rs; the arithmetic is in typing_stats.py.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import typing_stats  # noqa: E402


def main(argv):
    if len(argv) != 2:
        print(__doc__)
        return 2
    with open(argv[1], errors="replace") as f:
        events = typing_stats.parse_trace(f)
    if not events:
        print("no [ft] lines in the file: was the app started with ASHDASH_FRAME_TRACE=1 and stderr redirected to it?")
        return 1
    print(typing_stats.report(events))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

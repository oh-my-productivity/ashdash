#!/usr/bin/env bash
# The pure parts of the typing-lag measurement (scripts/qa/typing_stats.py): trace parsing, key-to-glyph and key-to-results latency,
# percentiles. No desktop: the module only reads text. Needs python3.
# policy-allow-file: heavy -- it names the module under scripts/qa and runs its pure unit tests (text parsing, no desktop)
set -u
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
cd "$here/../.." || exit 1
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest scripts/qa/typing_stats_test.py

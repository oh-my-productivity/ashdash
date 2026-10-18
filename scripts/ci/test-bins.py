#!/usr/bin/env python3
"""Print the test executables of a cargo workspace: one line per executable, `crate<TAB>kind<TAB>path`.

    cargo test --workspace --no-run --message-format=json | scripts/ci/test-bins.py

`kind` is `lib` for the unit tests of a crate (library or binary) and the target name for an integration test. Read from standard
input; used by test-times.sh and stress-timing.sh, which run the executables themselves (cwd = the crate folder, as cargo does).
"""
import json
import os
import sys

for line in sys.stdin:
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("reason") != "compiler-artifact" or not m.get("profile", {}).get("test") or not m.get("executable"):
        continue
    kinds = m["target"]["kind"]
    crate = os.path.basename(os.path.dirname(m["manifest_path"]))
    kind = "lib" if ("bin" in kinds or "lib" in kinds) else m["target"]["name"]
    print(f"{crate}\t{kind}\t{m['executable']}")

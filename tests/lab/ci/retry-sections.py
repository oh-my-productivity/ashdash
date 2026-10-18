#!/usr/bin/env python3
"""Which checklist sections must be re-run for the rows that FAILED?

usage: retry-sections.py CHECKLIST.sh RESULTS-FINAL.tsv      prints the section function names, one per line, in file order

Row ids are found in the bodies of the section functions of tests/lab/qa/checklist.sh (`result <id> ...`). GUI checks on a shared 3-core
virtual machine are timing sensitive; the CI wrapper re-runs a section once and reports both outcomes (docs/CI.md)."""
import re
import sys

SECTION = re.compile(r"^(s_setup|s_known|s\d+\w*|w45)\(\) \{")
RESULT = re.compile(r"\bresult ([A-Za-z0-9][A-Za-z0-9._-]*) ")


def main(argv):
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    owner = {}
    order = []
    current = None
    with open(argv[1], encoding="utf-8") as f:
        for line in f:
            m = SECTION.match(line)
            if m:
                current = m.group(1)
                order.append(current)
                continue
            if current:
                for rid in RESULT.findall(line):
                    owner.setdefault(rid, current)
    failed = []
    with open(argv[2], encoding="utf-8") as f:
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) >= 2 and parts[1] == "FAIL":
                failed.append(parts[0])
    wanted = {owner[r] for r in failed if r in owner}
    for s in order:
        if s in wanted:
            print(s)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

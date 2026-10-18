#!/usr/bin/env python3
"""Turn the raw result file of tests/lab/qa/{checklist,soak}.sh into the CI verdict.

usage: finalize.py RESULTS.tsv BLOCKED.tsv OUT.tsv SUMMARY.md [--require ID ...]

RESULTS.tsv   id <TAB> PASS|FAIL|BLOCKED|NEEDS-OWNER <TAB> evidence   (later lines override earlier ones with the same id)
BLOCKED.tsv   id-or-glob <TAB> reason   rows that cannot work on a GitHub-hosted runner; a FAIL on such a row becomes BLOCKED with the
              reason (the original evidence is kept). A PASS stays a PASS. Lines starting with # are comments.
Exit status: 1 when a row is still FAIL, when a --require'd id is missing or not PASS, or when the file has no rows at all."""
import fnmatch
import sys


def main(argv):
    args = argv[1:]
    if len(args) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    results, blocked, out_tsv, out_md = args[:4]
    required = []
    rest = args[4:]
    if rest:
        if rest[0] != "--require":
            print(__doc__, file=sys.stderr)
            return 2
        required = rest[1:]

    rules = []
    with open(blocked, encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if not line.strip() or line.startswith("#"):
                continue
            pat, _, why = line.partition("\t")
            rules.append((pat.strip(), why.strip()))

    rows = {}
    order = []
    try:
        with open(results, encoding="utf-8", errors="replace") as f:
            for line in f:
                parts = line.rstrip("\n").split("\t", 2)
                if len(parts) < 2 or not parts[0]:
                    continue
                rid, status = parts[0], parts[1]
                ev = parts[2] if len(parts) > 2 else ""
                if rid not in rows:
                    order.append(rid)
                rows[rid] = (status, ev)
    except FileNotFoundError:
        pass

    final = []
    for rid in order:
        status, ev = rows[rid]
        if status == "FAIL":
            for pat, why in rules:
                if fnmatch.fnmatchcase(rid, pat):
                    status = "BLOCKED"
                    ev = f"[hosted runner: {why}] (observed failure: {ev})"
                    break
        final.append((rid, status, ev))

    counts = {}
    for _, s, _ in final:
        counts[s] = counts.get(s, 0) + 1
    problems = []
    if not final:
        problems.append("no result rows at all (the script died early?)")
    by_id = {rid: s for rid, s, _ in final}
    for rid in required:
        if by_id.get(rid) != "PASS":
            problems.append(f"required row {rid} is {by_id.get(rid, 'missing')}")
    for rid, s, ev in final:
        if s == "FAIL":
            problems.append(f"{rid} FAIL: {ev[:200]}")

    with open(out_tsv, "w", encoding="utf-8") as f:
        for rid, s, ev in final:
            f.write(f"{rid}\t{s}\t{ev}\n")
    total = len(final)
    with open(out_md, "w", encoding="utf-8") as f:
        f.write(f"**{total} rows**: " + ", ".join(f"{n} {s}" for s, n in sorted(counts.items())) + "\n\n")
        shown = [r for r in final if r[1] != "PASS"]
        if shown:
            f.write("| row | status | evidence |\n|---|---|---|\n")
            for rid, s, ev in shown:
                ev = ev.replace("|", "\\|").replace("\n", " ")
                f.write(f"| {rid} | {s} | {ev[:300]} |\n")
        if problems:
            f.write("\n**Verdict: FAILED**\n\n" + "\n".join(f"* {p}" for p in problems) + "\n")
        else:
            f.write("\n**Verdict: OK** (no FAIL rows)\n")
    print(open(out_md, encoding="utf-8").read())
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

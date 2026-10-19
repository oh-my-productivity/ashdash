"""Pure parts of the typing-lag measurement (scripts/qa/perf-gui.py, PERF_MODE=typing): reading the launcher's frame trace and turning
the times at which the probe posted each key into "key to glyph drawn" and "key to results" latencies. No desktop, no clock, no I/O:
unit-tested by scripts/qa/typing_stats_test.py (run by tests/policy/typing-stats.test.sh).

The trace lines come from `ASHDASH_FRAME_TRACE=1` (crates/app/src/gui/frametrace.rs):

    [ft] t=1760870000123.4 request gen=3 q="pu"
    [ft] t=1760870000131.9 results gen=3 kind=Final rows=9
    [ft] t=1760870000125.0 frame dur=2.10 gap=16.7 text=1 q="pu" gen=3 rows=9 SLOW logic{poll_background=0.10} ui{keys=0.20 results=1.10}

`t` is the wall clock in Unix milliseconds. A frame line is written at the end of `ui()`, before the frame is rendered and presented, so
"key to glyph" is "key to the end of the frame that drew the glyph" and misses at most one display refresh (about 16 ms) of presentation.
"""
import re

_LINE = re.compile(r"\[ft\] t=(?P<t>[\d.]+) (?P<kind>request|results|frame)\b(?P<rest>.*)")
_Q = re.compile(r'\bq="((?:[^"\\]|\\.)*)"')
_FIELD = re.compile(r"\b(?P<k>dur|gap|text|gen|rows)=(?P<v>[\d.]+)")
_KIND = re.compile(r"\bkind=(\w+)")


def _unescape(s):
    return s.replace('\\"', '"').replace("\\\\", "\\")


def parse_trace(lines):
    """The trace events in `lines` (any other line is ignored): dicts with `type` (request|results|frame) and `t` (ms), plus
    `q`, `gen`, `kind`, `rows`, `dur`, `gap`, `text`, `slow` and the raw `line` where the line has them."""
    out = []
    for line in lines:
        m = _LINE.search(line)
        if not m:
            continue
        rest = m.group("rest")
        ev = {"type": m.group("kind"), "t": float(m.group("t")), "line": line.strip()}
        q = _Q.search(rest)
        if q:
            ev["q"] = _unescape(q.group(1))
        for f in _FIELD.finditer(rest):
            v = float(f.group("v"))
            ev[f.group("k")] = int(v) if f.group("k") in ("gen", "rows", "text") else v
        k = _KIND.search(rest)
        if k:
            ev["kind"] = k.group(1)
        ev["slow"] = " SLOW" in rest
        out.append(ev)
    return out


def percentile(values, p):
    """The p-th percentile (0..100) by the nearest-rank method; None for no values."""
    v = sorted(x for x in values if x is not None)
    if not v:
        return None
    rank = max(1, -(-len(v) * p // 100))  # ceil(len * p / 100)
    return v[int(rank) - 1]


def key_latencies(keys, events, text):
    """For each key `(index, char, t_ms)` the probe posted (typing `text`), the latency in ms of
    * `glyph`: from the key to the first traced frame at or after it whose query field already held `text[:index + 1]`,
    * `post_ms` (when the key tuple has a fourth item): how long the synthetic key press itself took to be handed over by the test driver;
      see [`POST_TAINTED_MS`],
    * `results`: from the key to the first `Final` results (or `Rerun`: any answer after the final one) of a query that was
      requested for exactly that prefix.
    A latency is None when nothing matching was traced (the glyph never drawn, the results never applied)."""
    requests = {e["gen"]: e["q"] for e in events if e["type"] == "request" and "gen" in e and "q" in e}
    out = []
    for key in keys:
        index, _char, t0 = key[:3]
        post_ms = key[3] if len(key) > 3 else None
        prefix = text[: index + 1]
        glyph = next(
            (e["t"] - t0 for e in events if e["type"] == "frame" and e["t"] >= t0 and e.get("q") == prefix),
            None,
        )
        results = next(
            (
                e["t"] - t0
                for e in events
                if e["type"] == "results"
                and e["t"] >= t0
                and e.get("kind") in ("Final", "Rerun")
                and requests.get(e.get("gen")) == prefix
            ),
            None,
        )
        row = {"index": index, "char": prefix[-1], "glyph_ms": _r(glyph), "results_ms": _r(results)}
        if post_ms is not None:
            row["post_ms"] = _r(post_ms)
        out.append(row)
    return out


# A key whose hand-over to the system took longer than this says nothing about the app: the driver (System Events) stalled, and the
# character reached the launcher that much later. Such keys are counted and left out of the percentiles.
POST_TAINTED_MS = 100.0


def _r(v):
    return None if v is None else round(v, 1)


def summarize(rows):
    """p50/p95/max/n of glyph_ms and results_ms over `rows` (the dicts of [`key_latencies`], any number of runs), overall and
    per character position. Keys that the test driver handed over late (`post_ms` over [`POST_TAINTED_MS`]) are counted in `tainted`
    and not measured."""
    tainted = [r for r in rows if r.get("post_ms", 0.0) > POST_TAINTED_MS]
    rows = [r for r in rows if r.get("post_ms", 0.0) <= POST_TAINTED_MS]

    def stats(values):
        got = [v for v in values if v is not None]
        return {
            "n": len(got),
            "missing": len(values) - len(got),
            "p50": percentile(got, 50),
            "p95": percentile(got, 95),
            "max": max(got) if got else None,
        }

    out = {"all": {m: stats([r[m] for r in rows]) for m in ("glyph_ms", "results_ms")}, "by_index": {}, "tainted": len(tainted)}
    for index in sorted({r["index"] for r in rows}):
        sel = [r for r in rows if r["index"] == index]
        out["by_index"][index] = {m: stats([r[m] for r in sel]) for m in ("glyph_ms", "results_ms")}
    return out


def slow_frames(events, since_ms=0.0, limit=5):
    """The `limit` slowest frames (by `dur`) at or after `since_ms`, as their raw trace lines."""
    frames = [e for e in events if e["type"] == "frame" and e["t"] >= since_ms]
    frames.sort(key=lambda e: e.get("dur", 0.0), reverse=True)
    return [e["line"] for e in frames[:limit]]


def report(events, stall_ms=30.0):
    """A plain-text report of one session's trace for someone without a probe (`scripts/qa/frame-trace-report.py`): for every typed
    character the frame that drew it, how long the app took from the request to the answer, every slow frame with its phases, and the
    gaps between frames while text was typed. A frame's `gap` is the time since the previous frame started: a long one right before a
    typed character, with a short `dur`, means the thread was blocked outside the app's own code (waiting for the event, presenting the
    frame), a long `dur` means the app's own code was slow (the phases say which part)."""
    lines = []
    requests = {e["gen"]: e for e in events if e["type"] == "request" and "gen" in e}
    answered = {}
    for e in events:
        if e["type"] == "results" and e.get("kind") in ("Final", "Rerun") and e.get("gen") in requests:
            answered.setdefault(e["gen"], e)
    typed = [e for e in events if e["type"] == "frame" and e.get("text", 0) > 0]
    lines.append(f"{len(typed)} frames drew typed text, {len(requests)} queries were requested")
    lines.append("")
    lines.append("typed text: query field | frame dur | gap before the frame | request -> answer")
    for e in typed:
        req, ans = requests.get(e.get("gen")), answered.get(e.get("gen"))
        took = "no answer traced" if ans is None or req is None else f"{ans['t'] - req['t']:.1f} ms"
        mark = "  <-- long gap" if e.get("gap", 0.0) >= stall_ms else ""
        lines.append(f"  {e.get('q', '')!r:<12} dur {e.get('dur', 0.0):6.2f} ms   gap {e.get('gap', 0.0):7.1f} ms   answer {took}{mark}")
    slow = [e for e in events if e["type"] == "frame" and e["slow"]]
    lines.append("")
    lines.append(f"{len(slow)} slow frames (over 16 ms of the app's own work):")
    for e in sorted(slow, key=lambda e: -e.get("dur", 0.0))[:10]:
        lines.append("  " + e["line"].split("] ", 1)[-1])
    return "\n".join(lines)


# Generous limits (the CI runner is a shared virtual machine): a typed character is drawn within about three 60 Hz frames, and the
# answer of a query that only reads memory arrives within 150 ms; the slow variant runs two Script Filters of 250 ms each, which side by
# side take about 300 ms and one after the other 550 ms.
GLYPH_P95_MS = 50.0
RESULTS_P95_MS = {"heavy": 150.0, "heavy-slow": 450.0}


def check_limits(summary, variant, glyph_p95=GLYPH_P95_MS, results_p95=None):
    """The ways `summary` (the dict of [`summarize`]) breaks the limits, as sentences; empty = fine. Missing data is a failure: a run
    that measured nothing must not pass."""
    limit_results = (results_p95 or RESULTS_P95_MS).get(variant)
    out = []
    g = summary["all"]["glyph_ms"]
    if g["n"] == 0:
        out.append(f"{variant}: no key-to-glyph latency was measured")
    elif g["p95"] > glyph_p95:
        out.append(f"{variant}: key-to-glyph p95 {g['p95']} ms is over {glyph_p95} ms")
    if g["n"] and g["missing"] > g["n"] // 2:
        out.append(f"{variant}: {g['missing']} of {g['n'] + g['missing']} typed characters were never seen drawn")
    tainted = summary.get("tainted", 0)
    if tainted > (g["n"] + tainted) // 3:
        out.append(f"{variant}: the test driver stalled on {tainted} of {g['n'] + tainted} keys: the run says nothing")
    r = summary["all"]["results_ms"]
    if limit_results is not None:
        if r["n"] == 0:
            out.append(f"{variant}: no key-to-results latency was measured")
        elif r["p95"] > limit_results:
            out.append(f"{variant}: key-to-results p95 {r['p95']} ms is over {limit_results} ms")
    return out

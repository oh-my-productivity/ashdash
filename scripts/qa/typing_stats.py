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
    * `results`: from the key to the first `Final` results (or `Rerun`: any answer after the final one) of a query that was
      requested for exactly that prefix.
    A latency is None when nothing matching was traced (the glyph never drawn, the results never applied)."""
    requests = {e["gen"]: e["q"] for e in events if e["type"] == "request" and "gen" in e and "q" in e}
    out = []
    for index, _char, t0 in keys:
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
        out.append({"index": index, "char": prefix[-1], "glyph_ms": _r(glyph), "results_ms": _r(results)})
    return out


def _r(v):
    return None if v is None else round(v, 1)


def summarize(rows):
    """p50/p95/max/n of glyph_ms and results_ms over `rows` (the dicts of [`key_latencies`], any number of runs), overall and
    per character position."""

    def stats(values):
        got = [v for v in values if v is not None]
        return {
            "n": len(got),
            "missing": len(values) - len(got),
            "p50": percentile(got, 50),
            "p95": percentile(got, 95),
            "max": max(got) if got else None,
        }

    out = {"all": {m: stats([r[m] for r in rows]) for m in ("glyph_ms", "results_ms")}, "by_index": {}}
    for index in sorted({r["index"] for r in rows}):
        sel = [r for r in rows if r["index"] == index]
        out["by_index"][index] = {m: stats([r[m] for r in sel]) for m in ("glyph_ms", "results_ms")}
    return out


def slow_frames(events, since_ms=0.0, limit=5):
    """The `limit` slowest frames (by `dur`) at or after `since_ms`, as their raw trace lines."""
    frames = [e for e in events if e["type"] == "frame" and e["t"] >= since_ms]
    frames.sort(key=lambda e: e.get("dur", 0.0), reverse=True)
    return [e["line"] for e in frames[:limit]]

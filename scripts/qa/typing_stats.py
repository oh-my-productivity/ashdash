"""Pure parts of the typing-lag measurement (scripts/qa/perf-gui.py, PERF_MODE=typing): reading the launcher's frame trace and turning
the times at which the probe posted each key into "key to glyph drawn" and "key to results" latencies. No desktop, no clock, no I/O:
unit-tested by scripts/qa/typing_stats_test.py (run by tests/policy/typing-stats.test.sh).

The trace lines come from `ASHDASH_FRAME_TRACE=1` (crates/app/src/gui/frametrace.rs):

    [ft] t=1760870000123.4 request gen=3 q="pu"
    [ft] t=1760870000131.9 results gen=3 kind=Final rows=9
    [ft] t=1760870000125.0 frame dur=2.10 gap=16.7 text=1 q="pu" gen=3 rows=9 SLOW logic{poll_background=0.10} ui{keys=0.20 results=1.10}
    [ft] t=1760870000124.9 event key down code=35 age=1.8 slept_before=412.0 awake_for=0.1 prev_busy=0.4
    [ft] t=1760870001125.0 loop busy=1003.1

`event` lines (a key reached the app, `age` = ms since the window server made the event) and `loop busy` lines (the main thread stayed out
of its run loop that long) come from crates/app/src/gui/looptrace.rs; they separate "the OS delivered the key late" from "the app was blocked".

`t` is the wall clock in Unix milliseconds. A frame line is written at the end of `ui()`, before the frame is rendered and presented, so
"key to glyph" is "key to the end of the frame that drew the glyph" and misses at most one display refresh (about 16 ms) of presentation.
"""
import re

_LINE = re.compile(r"\[ft\] t=(?P<t>[\d.]+) (?P<kind>request|results|frame|event|loop|stall-sample)\b(?P<rest>.*)")
_Q = re.compile(r'\bq="((?:[^"\\]|\\.)*)"')
_FIELD = re.compile(r"\b(?P<k>dur|gap|text|gen|rows|age|code|busy|slept_before|awake_for|prev_busy)=(?P<v>[\d.]+)")
_KIND = re.compile(r"\bkind=(\w+)")
_KEY = re.compile(r"^\s*key (\w+)")


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
        kk = _KEY.search(rest)
        if kk:
            ev["key"] = kk.group(1)
        ev["slow"] = " SLOW" in rest
        out.append(ev)
    return out


# a `loop busy` stretch shorter than this is ordinary frame work
BUSY_MIN_MS = 100.0


def percentile(values, p):
    """The p-th percentile (0..100) by the nearest-rank method; None for no values."""
    v = sorted(x for x in values if x is not None)
    if not v:
        return None
    rank = max(1, -(-len(v) * p // 100))  # ceil(len * p / 100)
    return v[int(rank) - 1]


def key_latencies(keys, events, text):
    """For each key `(index, char, t_ms)` the probe posted (typing `text`), the latency in ms of
    * `glyph`: from the key to the first traced frame at or after it whose query field already held `text[:index + 1]` (or more: when the
      main thread was blocked, several keys are drawn by one frame),
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
            (e["t"] - t0 for e in events if e["type"] == "frame" and e["t"] >= t0 and e.get("q", "").startswith(prefix)),
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


def event_delays(events):
    """For each key-down `event` line (a key reached the app, see looptrace.rs): `age` (ms from the window server's own timestamp to the
    app's first look at it) and `to_frame` (ms from that look to the end of the first frame that drew typed text, None when none followed).
    `age` large = the key waited before the app saw it (OS queue or a blocked main thread); `to_frame` large = the app saw it and drew late."""
    frames = [e for e in events if e["type"] == "frame" and e.get("text", 0) > 0]
    out = []
    for e in events:
        if e["type"] != "event" or e.get("key") != "down":
            continue
        f = next((f for f in frames if f["t"] >= e["t"]), None)
        out.append({"t": e["t"], "age": e.get("age"), "slept_before": e.get("slept_before"), "prev_busy": e.get("prev_busy"),
                    "to_frame": None if f is None else _r(f["t"] - e["t"])})
    return out


def busy_stretches(events, min_ms=BUSY_MIN_MS):
    """The `loop busy` lines of at least `min_ms`: stretches in which the main thread did not return to its run loop."""
    return [e for e in events if e["type"] == "loop" and e.get("busy", 0.0) >= min_ms]


def verdict(events, late_ms=200.0):
    """One sentence on what the trace shows about late keys, for the report. A key is late when the app saw it `late_ms` or more after the
    window server made it (`age`). If a `loop busy` stretch of about that length ended right before, the main thread was blocked (the paint of
    a frame runs inside such a stretch); if the loop slept through it, the key or its wake-up reached the app late from outside."""
    delays = event_delays(events)
    if not delays:
        return "no key events were traced: this trace comes from a build without `event` lines, or no key was typed"
    late = [d for d in delays if (d["age"] or 0.0) >= late_ms]
    if not late:
        return f"{len(delays)} keys, none reached the app later than {late_ms:.0f} ms after the window server made it: no late key in this trace"
    busy = busy_stretches(events, late_ms)
    if busy:
        return (f"{len(late)} of {len(delays)} keys waited {late_ms:.0f} ms or more before the app saw them, and the main thread was blocked "
                f"for {len(busy)} stretches of {min(b['busy'] for b in busy):.0f}-{max(b['busy'] for b in busy):.0f} ms: the app was not "
                "looking at its events, the keys were not delayed on the way")
    return (f"{len(late)} of {len(delays)} keys waited {late_ms:.0f} ms or more before the app saw them while the main thread was not busy: "
            "the loop slept and was woken late (App Nap / timer coalescing / the OS)")


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
    delays = event_delays(events)
    if delays:
        lines.append("")
        lines.append("key events: age = window server -> app saw it | to_frame = app saw it -> frame that drew it ended")
        for d in delays:
            mark = "  <-- late" if (d["age"] or 0.0) >= stall_ms or (d["to_frame"] or 0.0) >= stall_ms else ""
            lines.append(f"  age {d['age']!s:>7} ms   to_frame {d['to_frame']!s:>7} ms   slept_before {d['slept_before']!s:>8}{mark}")
    busy = busy_stretches(events, 50.0)
    if busy:
        lines.append("")
        lines.append(f"{len(busy)} stretches of 50 ms or more in which the main thread did not return to its run loop (`loop busy`):")
        for b in busy[:20]:
            lines.append(f"  t={b['t']:.1f}  busy {b['busy']:.1f} ms")
    lines.append("")
    lines.append("verdict: " + verdict(events))
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


# --- typing after a pause (PERF_MODE=idle): the late-key bug of the owner's laptop, where a key typed after the launcher sat idle was
# drawn up to a second late. Rows are the dicts of [`key_latencies`] with `idle_s` (the pause before the burst) added.

# a key drawn this long after it was typed is a stall (the bug is a ~1000 ms one); the pass criterion is that none happen
STALL_MS = 500.0
IDLE_GLYPH_P95_MS = 50.0


def summarize_idle(rows):
    """Per pause length: p50/p95/max of the key-to-glyph latency of the keys typed after the pause, how many were drawn `STALL_MS` or
    later (`stalls`), how many never were (`missing`), and how many the test driver handed over late (`tainted`, left out)."""
    out = {}
    for idle in sorted({r["idle_s"] for r in rows}):
        sel = [r for r in rows if r["idle_s"] == idle]
        ok = [r for r in sel if r.get("post_ms", 0.0) <= POST_TAINTED_MS]
        got = [r["glyph_ms"] for r in ok if r["glyph_ms"] is not None]
        out[idle] = {
            "n": len(got),
            "missing": len(ok) - len(got),
            "tainted": len(sel) - len(ok),
            "p50": percentile(got, 50),
            "p95": percentile(got, 95),
            "max": max(got) if got else None,
            "stalls": sum(1 for g in got if g >= STALL_MS),
        }
    return out


def check_idle_limits(summary, p95_limit=IDLE_GLYPH_P95_MS, stall_limit=STALL_MS):
    """The ways the per-pause `summary` breaks the limits (p95 under `p95_limit`, no key drawn `stall_limit` or later); empty = fine.
    No data is a failure."""
    out = []
    if not summary:
        return ["idle: no key was measured"]
    for idle, s in summary.items():
        if s["n"] == 0:
            out.append(f"idle {idle:g} s: no key-to-glyph latency was measured")
            continue
        if s["p95"] > p95_limit:
            out.append(f"idle {idle:g} s: key-to-glyph p95 {s['p95']} ms is over {p95_limit:g} ms")
        if s["max"] >= stall_limit:
            out.append(f"idle {idle:g} s: {s['stalls']} of {s['n']} keys were drawn {stall_limit:g} ms or later (max {s['max']} ms)")
        if s["missing"] > s["n"] // 2:
            out.append(f"idle {idle:g} s: {s['missing']} of {s['n'] + s['missing']} keys were never seen drawn")
    return out

#!/usr/bin/env python3
"""Turn the results.jsonl of scripts/qa/perf-gui.sh into a Markdown table for the job summary.

    perf-summary.py results.jsonl

One row per measurement kind and variant: start-up (cold and warm cache), idle CPU and memory with the window hidden, memory after use,
hotkey-to-window and `ashdash show` latency. The numbers are measurements on a shared virtual machine: use them to compare runs of the
same image over time and for orders of magnitude, not as absolute limits (docs/PERF.md).
"""
import json
import sys


def med(values):
    values = sorted(v for v in values if v is not None)
    return values[len(values) // 2] if values else None


def fmt(v, unit=""):
    return "-" if v is None else f"{v:g}{unit}"


def main(path):
    rows = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            try:
                rows.append(json.loads(line))
            except ValueError:
                continue
    out = []
    env = next((r for r in rows if r.get("kind") == "env"), {})
    out.append(f"Accessibility trusted: `{env.get('accessibility_trusted', '?')}`; apps per variant: `{env.get('apps', '?')}`\n")

    out.append("| start-up (median over repetitions) | variant | socket ms | hotkey ready ms | first window ms | hotkey to window ms |")
    out.append("|---|---|---:|---:|---:|---:|")
    keys = sorted({(r["variant"], r["phase"]) for r in rows if r.get("kind") == "start" and "variant" in r})
    for variant, phase in keys:
        sel = [r for r in rows if r.get("kind") == "start" and r.get("variant") == variant and r.get("phase") == phase]
        out.append(
            f"| {phase} | {variant} | {fmt(med([r.get('socket_ms') for r in sel]))} | {fmt(med([r.get('hotkey_ready_ms') for r in sel]))} | "
            f"{fmt(med([r.get('first_window_ms') for r in sel]))} | {fmt(med([r.get('hotkey_to_window_ms') for r in sel]))} |"
        )

    out.append("\n| idle, window hidden | variant | RSS MB | footprint MB | CPU % | over s |")
    out.append("|---|---|---:|---:|---:|---:|")
    for variant in sorted({r["variant"] for r in rows if r.get("kind") == "idle"}):
        sel = [r for r in rows if r.get("kind") == "idle" and r["variant"] == variant]
        out.append(
            f"| {sel[0].get('tag', 'idle')} | {variant} | {fmt(med([r['snap_end']['rss_mb'] for r in sel]))} | "
            f"{fmt(med([r['snap_end']['footprint_mb'] for r in sel]))} | {fmt(med([r.get('cpu_pct') for r in sel]))} | {fmt(med([r.get('over_s') for r in sel]))} |"
        )

    out.append("\n| latency | variant | n | min ms | median ms | p95 ms | max ms |")
    out.append("|---|---|---:|---:|---:|---:|---:|")
    for variant in sorted({r["variant"] for r in rows if r.get("kind") == "latency"}):
        for r in [r for r in rows if r.get("kind") == "latency" and r["variant"] == variant]:
            for label, key in (("hotkey", "hotkey_ms"), ("ashdash show", "ipc_show_ms")):
                s = r.get(key)
                if s:
                    out.append(f"| {label} (rep {r['rep']}) | {variant} | {s['n']} | {s['min']} | {s['median']} | {s['p95']} | {s['max']} |")

    out.append("\n| after use | variant | RSS MB before | RSS MB after cycles | footprint MB after cycles |")
    out.append("|---|---|---:|---:|---:|")
    for r in rows:
        if r.get("kind") == "cycles":
            out.append(f"| {r['n']} show/type/hide cycles (rep {r['rep']}) | {r['variant']} | {fmt(r['baseline']['rss_mb'])} | {fmt(r['snap']['rss_mb'])} | {fmt(r['snap']['footprint_mb'])} |")
    print("\n".join(out))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    main(sys.argv[1])

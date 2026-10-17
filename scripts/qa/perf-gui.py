#!/usr/bin/env python3
"""Measure the real release `ashdash` launcher process (hidden egui window, GUI daemon).

Do not run this directly: scripts/qa/perf-gui.sh takes the desktop lock, makes a temporary HOME
and cleans up. This file only measures. It never touches the user's data folder: HOME is the
temporary directory it is given, and the app list is taken from `app_dirs` in that HOME's config.

Per variant (apps=empty | real | synth5000) and per repetition:
  1. cold start (no cache): spawn, time to "hotkey ready" (the launcher's own stderr line
     "press <hotkey> to open", printed right after the hotkey is registered) and time to the control
     socket. Then post the real hotkey at once and time until a launcher-sized window is on screen
     ("first window shown", includes the first frame on a cold GPU path).
  2. hide (Esc), settle, then idle RSS / footprint / CPU over IDLE_S seconds with the window hidden.
  3. cycle: show (hotkey), type a query (synthetic key events), hide (Esc): RSS/footprint after,
     +10 s, and after N more cycles (does memory come back, does it grow?).
  4. hotkey -> window visible latency, N presses (also `ashdash show` over the socket as a proxy).
  5. quit, start again with the cache now warm: hotkey-ready and first-window times.
Results go to $PERF_OUT/results.jsonl, one JSON object per measurement.
"""
import json, os, re, signal, subprocess, sys, threading, time, statistics, shutil

BIN = os.environ["PERF_BIN"]
PROBE = os.environ["PERF_PROBE"]
OUT = os.environ["PERF_OUT"]
ROOT = os.environ["PERF_ROOT"]          # temp dir holding the per-run HOMEs and the synthetic apps
IDLE_S = int(os.environ.get("PERF_IDLE_S", "60"))
REPS = int(os.environ.get("PERF_REPS", "3"))
REP_START = int(os.environ.get("PERF_REP_START", "1"))   # numbering only; lets the lock be released between reps
CYCLES = int(os.environ.get("PERF_CYCLES", "20"))
LAT_N = int(os.environ.get("PERF_LAT_N", "10"))
VARIANTS = os.environ.get("PERF_VARIANTS", "empty,real,synth5000").split(",")
QUERY = os.environ.get("PERF_QUERY", "saf")
HOTKEY = ("ctrl+alt+shift+j", "38", "alt,shift,ctrl")   # config string, key code, flags
OWNER = "ashdash"
MINW = "300"       # launcher window is >= 300 pt wide; the status item window is ~22 pt


def log(**kw):
    kw["t"] = time.time()
    with open(os.path.join(OUT, "results.jsonl"), "a") as f:
        f.write(json.dumps(kw) + "\n")
    print(json.dumps(kw), flush=True)


def sh(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


def load():
    up = sh(["uptime"]).stdout.strip()
    ps = sh(["ps", "-Ao", "pcpu"]).stdout.split()[1:]
    return {"uptime": up.split("load averages:")[-1].strip(), "others_pcpu_sum": round(sum(float(x) for x in ps), 1)}


def cputime(pid):
    out = sh(["ps", "-o", "cputime=", "-p", str(pid)]).stdout.strip()
    if not out:
        return None
    parts = out.split(":")
    sec = 0.0
    for p in parts:
        sec = sec * 60 + float(p)
    return sec


def rss_mb(pid):
    out = sh(["ps", "-o", "rss=", "-p", str(pid)]).stdout.strip()
    return int(out) / 1024 if out else None


def footprint_mb(pid):
    """Physical footprint (what Activity Monitor calls Memory), MB."""
    out = sh(["footprint", "-p", str(pid)]).stdout
    m = re.search(r"Footprint:\s+([\d.]+)\s*([KMG]B)", out)
    if not m:
        m = re.search(r"phys_footprint:\s+([\d.]+)\s*([KMG]B)", out)
    if not m:
        return None
    v = float(m.group(1))
    return v * {"KB": 1 / 1024, "MB": 1, "GB": 1024}[m.group(2)]


def threads(pid):
    return len(sh(["ps", "-M", "-p", str(pid)]).stdout.strip().splitlines()) - 1


class Instance:
    def __init__(self, home):
        self.home = home
        env = dict(os.environ, HOME=home, ASHDASH_TRACE="1")
        for k in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
            env.pop(k, None)
        self.lines = []        # (monotonic time, text)
        self.t0 = time.time()
        self.p = subprocess.Popen([BIN], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                  stderr=subprocess.PIPE, text=True, start_new_session=True)
        self.pid = self.p.pid
        threading.Thread(target=self._read, daemon=True).start()
        self.sock = os.path.join(home, "Library/Application Support/ashdash/run/ipc.sock")

    def _read(self):
        for line in self.p.stderr:
            self.lines.append((time.time(), line.rstrip()))

    def wait_line(self, pat, timeout=30, after=0.0):
        end = time.time() + timeout
        while time.time() < end:
            for t, l in list(self.lines):
                if t >= after and re.search(pat, l):
                    return t
            if self.p.poll() is not None:
                return None
            time.sleep(0.001)
        return None

    def wait_socket(self, timeout=30):
        end = time.time() + timeout
        while time.time() < end:
            if os.path.exists(self.sock):
                return time.time()
            time.sleep(0.001)
        return None

    def alive(self):
        return self.p.poll() is None

    def quit(self):
        if self.alive():
            try:
                sh([BIN, "quit"], env=dict(os.environ, HOME=self.home), timeout=10)
            except Exception:
                pass
            for _ in range(50):
                if not self.alive():
                    break
                time.sleep(0.1)
        if self.alive():
            self.p.kill()
        try:
            self.p.wait(timeout=5)
        except Exception:
            pass


def probe(*args, timeout=15):
    r = sh([PROBE, *map(str, args)], timeout=timeout)
    return r.returncode, r.stdout.strip()


def window_visible_after(since, timeout_ms=3000):
    rc, out = probe("visible", OWNER, MINW, timeout_ms)
    return float(out.split()[1]) if rc == 0 else None


LAST_POST = [0.0]
LAST_POSTED_MS = [0.0]   # how long the probe itself needed to build and post the events (harness cost inside the latency)


def hotkey_show(timeout_ms=3000):
    """Post the hotkey; return latency ms to a launcher-sized window on screen (None = never)."""
    rc, out = probe("hotkey", HOTKEY[1], HOTKEY[2], OWNER, MINW, timeout_ms)
    m = re.search(r"latency_ms ([\d.]+)", out)
    p = re.search(r"post ([\d.]+)", out)
    LAST_POST[0] = float(p.group(1)) if p else 0.0
    q = re.search(r"posted_ms ([\d.]+)", out)
    LAST_POSTED_MS[0] = float(q.group(1)) if q else 0.0
    return float(m.group(1)) if (rc == 0 and m) else None


def is_visible():
    rc, _ = probe("visible", OWNER, MINW, 5)
    return rc == 0


def hide(inst=None):
    """Hide the launcher by pressing the hotkey again (a registered global hotkey is consumed by the
    launcher, so unlike Esc it can never leak into another app). No-op when already hidden."""
    if not is_visible():
        return True
    probe("hotkey", HOTKEY[1], HOTKEY[2], OWNER, MINW, 5)  # 'visible' returns at once; we only want the chord posted
    rc, _ = probe("hidden", OWNER, MINW, 3000)
    return rc == 0


def focused_since(inst, since, timeout=3.0):
    """True when the launcher's own trace reports focus after `since` (typed text would reach it)."""
    return inst.wait_line(r"focus=Some\(true\)", timeout=timeout, after=since) is not None


def type_safely(inst, since, text):
    if not focused_since(inst, since):
        return False
    probe("type", text)
    return True


def dump(inst, variant, rep, name):
    """Keep `footprint` and `vmmap -summary` text for later analysis (no user data in it: temp HOME only)."""
    d = os.path.join(OUT, "mem")
    os.makedirs(d, exist_ok=True)
    for tool, args in (("footprint", ["footprint", "-p", str(inst.pid)]), ("vmmap", ["vmmap", "-summary", str(inst.pid)])):
        r = sh(args)
        with open(os.path.join(d, f"{variant}-{rep}-{name}.{tool}.txt"), "w") as f:
            f.write(r.stdout + r.stderr)


def snapshot(inst, label):
    return {"label": label, "rss_mb": round(rss_mb(inst.pid) or -1, 1),
            "footprint_mb": round(footprint_mb(inst.pid) or -1, 1), "threads": threads(inst.pid)}


def make_home(variant, tag):
    home = os.path.join(ROOT, f"h-{variant}-{tag}")
    shutil.rmtree(home, ignore_errors=True)
    data = os.path.join(home, "Library/Application Support/ashdash")
    os.makedirs(data)
    cfg = {"hotkey": HOTKEY[0]}
    if variant == "empty":
        cfg["app_dirs"] = []
    elif variant == "synth5000":
        cfg["app_dirs"] = [os.path.join(ROOT, "apps5000")]
    elif variant != "real":
        raise SystemExit(f"unknown variant {variant}")
    with open(os.path.join(data, "config.json"), "w") as f:
        json.dump(cfg, f)
    return home


def count_apps(variant):
    if variant == "empty":
        return 0
    if variant == "synth5000":
        return len([x for x in os.listdir(os.path.join(ROOT, "apps5000")) if x.endswith(".app")])
    n = 0
    for d in ("/Applications", "/System/Applications", "/System/Library/CoreServices/Applications"):
        if os.path.isdir(d):
            n += len([x for x in os.listdir(d) if x.endswith(".app")])
    return n


def start_timing(variant, rep, home, phase):
    """Spawn, time hotkey-ready, socket, and first window via the real hotkey."""
    if not os.path.exists(BIN):
        raise SystemExit("no binary")
    before = load()
    inst = Instance(home)
    t_sock = inst.wait_socket()
    t_ready = inst.wait_line(r"press .* to open", timeout=30)
    res = {"variant": variant, "rep": rep, "phase": phase, "load_before": before,
           "socket_ms": round((t_sock - inst.t0) * 1000, 1) if t_sock else None,
           "hotkey_ready_ms": round((t_ready - inst.t0) * 1000, 1) if t_ready else None}
    if t_ready is None:
        res["error"] = "no hotkey-ready line; stderr: " + " | ".join(l for _, l in inst.lines[-5:])
        log(kind="start", **res)
        return inst, res
    # Press the real hotkey immediately: first window shown.
    t_press = time.time()
    rc, out = probe("hotkey", HOTKEY[1], HOTKEY[2], OWNER, MINW, 5000)
    m = re.search(r"visible ([\d.]+)", out)
    if rc == 0 and m:
        t_vis = float(m.group(1))
        res["first_window_ms"] = round((t_vis - inst.t0) * 1000, 1)
        res["hotkey_to_window_ms"] = round((t_vis - t_press) * 1000, 1)
    else:
        res["first_window_ms"] = None
        res["first_window_note"] = "no window within 5 s after posting the hotkey: " + out
    t_foc = inst.wait_line(r"focus=Some\(true\)", timeout=3)
    res["focus_ms"] = round((t_foc - inst.t0) * 1000, 1) if t_foc else None
    res["load_after"] = load()
    log(kind="start", **res)
    return inst, res


def idle_block(inst, variant, rep, tag):
    time.sleep(10)  # let start-up work (refresh, icon, watchers) settle
    s0 = snapshot(inst, "idle+10s")
    c0, w0 = cputime(inst.pid), time.time()
    time.sleep(IDLE_S)
    c1, w1 = cputime(inst.pid), time.time()
    s1 = snapshot(inst, f"idle+{10 + IDLE_S}s")
    dump(inst, variant, rep, "idle-hidden")
    # idle wakeups from top (one 5 s interval; the first sample is since-start and discarded)
    top = sh(["top", "-l", "2", "-s", "5", "-pid", str(inst.pid), "-stats", "pid,cpu,idlew,threads"]).stdout
    last = [l for l in top.splitlines() if l.strip().startswith(str(inst.pid))]
    log(kind="idle", variant=variant, rep=rep, tag=tag, window="hidden", snap_start=s0, snap_end=s1,
        cpu_seconds=round(c1 - c0, 3), over_s=round(w1 - w0, 1),
        cpu_pct=round(100 * (c1 - c0) / (w1 - w0), 3), top_last=last[-1].split() if last else None,
        load=load())


def cycle_block(inst, variant, rep):
    base = snapshot(inst, "before-show")
    t_show = time.time()
    ok = hotkey_show()
    time.sleep(0.5)
    shown = snapshot(inst, "window-shown")
    typed_ok = type_safely(inst, t_show, QUERY)
    time.sleep(0.8)
    typed = snapshot(inst, f"typed '{QUERY}' ok={typed_ok}")
    dump(inst, variant, rep, "window-shown-typed")
    hidden_ok = hide(inst)
    time.sleep(1)
    h1 = snapshot(inst, "hidden+1s")
    time.sleep(10)
    h10 = snapshot(inst, "hidden+11s")
    dump(inst, variant, rep, "hidden-after-use")
    log(kind="cycle1", variant=variant, rep=rep, shown_ok=ok is not None, typed_ok=typed_ok, hidden_ok=hidden_ok,
        snaps=[base, shown, typed, h1, h10])
    # N more cycles: show, type, hide
    for _ in range(CYCLES):
        t = time.time()
        hotkey_show()
        type_safely(inst, t, QUERY)
        time.sleep(0.4)
        hide(inst)
        time.sleep(0.3)
    time.sleep(5)
    after = snapshot(inst, f"after {CYCLES} more cycles +5s")
    log(kind="cycles", variant=variant, rep=rep, n=CYCLES, snap=after, baseline=base)


def latency_block(inst, variant, rep):
    hot, ipc, to_show, to_focus = [], [], [], []
    for _ in range(LAT_N):
        v = hotkey_show()
        if v is not None:
            hot.append(v)
            tp = LAST_POST[0]
            ts = inst.wait_line(r"\] show$", timeout=1, after=tp)
            tf = inst.wait_line(r"focus=Some\(true\)", timeout=1, after=tp)
            if ts:
                to_show.append((ts - tp) * 1000)
            if tf:
                to_focus.append((tf - tp) * 1000)
        time.sleep(0.4)
        hide(inst)
        time.sleep(0.4)
    for _ in range(LAT_N):
        # start the window watcher first so the CLI's own exit time is not part of the number
        w = subprocess.Popen([PROBE, "visible", OWNER, MINW, "3000"], stdout=subprocess.PIPE, text=True)
        time.sleep(0.05)
        t0 = time.time()
        sh([BIN, "show"], env=dict(os.environ, HOME=inst.home))
        out = w.communicate()[0].split()
        if w.returncode == 0 and len(out) == 2:
            ipc.append((float(out[1]) - t0) * 1000)
        time.sleep(0.4)
        hide(inst)
        time.sleep(0.4)

    def stats(a):
        if not a:
            return None
        s = sorted(a)
        return {"n": len(a), "min": round(s[0], 1), "median": round(statistics.median(s), 1),
                "p95": round(s[min(len(s) - 1, int(0.95 * len(s)))], 1), "max": round(s[-1], 1)}
    log(kind="latency", variant=variant, rep=rep, hotkey_ms=stats(hot), hotkey_all=[round(x, 1) for x in hot],
        ipc_show_ms=stats(ipc), ipc_all=[round(x, 1) for x in ipc],
        post_to_show_trace_ms=stats(to_show), post_to_focus_trace_ms=stats(to_focus),
        note="hotkey = synthetic CGEvent chord posted to the HID tap until a launcher-sized window is in the on-screen "
             "window list (1 ms polling); ipc = `ashdash show` process spawn + socket round trip + window visible")


def gap_mode():
    """Hotkey and `ashdash show` latency as a function of the time the launcher has been hidden
    (is it App Nap / idle throttling?), plus the window size while shown."""
    home = make_home("empty", "gap")
    inst, res = start_timing("empty", 0, home, "gap-start")
    hide(inst)
    for gap in [float(x) for x in os.environ.get("PERF_GAPS", "0.5,3,10,30").split(",")]:
        for i in range(3):
            hide(inst)
            time.sleep(gap)
            for how in ("hotkey", "ipc"):
                if how == "hotkey":
                    tp0 = time.time()
                    v = hotkey_show()
                    tp = LAST_POST[0]
                else:
                    w = subprocess.Popen([PROBE, "visible", OWNER, MINW, "3000"], stdout=subprocess.PIPE, text=True)
                    time.sleep(0.05)
                    tp = time.time()
                    sh([BIN, "show"], env=dict(os.environ, HOME=inst.home))
                    o = w.communicate()[0].split()
                    v = (float(o[1]) - tp) * 1000 if (w.returncode == 0 and len(o) == 2) else None
                ts = inst.wait_line(r"\] show$", timeout=1, after=tp)
                tf = inst.wait_line(r"focus=Some\(true\)", timeout=1, after=tp)
                bounds = probe("bounds", OWNER)[1].replace("\n", " | ") if how == "hotkey" and i == 0 else None
                log(kind="gap", gap_s=gap, i=i, how=how, to_visible_ms=round(v, 1) if v else None,
                    to_show_trace_ms=round((ts - tp) * 1000, 1) if ts else None,
                    to_focus_trace_ms=round((tf - tp) * 1000, 1) if tf else None, bounds=bounds)
                time.sleep(0.5)
                hide(inst)
                time.sleep(gap if how == "hotkey" else 0)
    inst.quit()
    shutil.rmtree(home, ignore_errors=True)


def quick_mode(mode):
    """PERF_MODE=latency | mem: one short run per variant/rep (about 40 s), so the desktop lock stays short.
    latency: hotkey and `ashdash show` -> window visible. mem: footprint before, with the window shown (typed), and after hiding."""
    for variant in VARIANTS:
        for rep in range(REP_START, REP_START + REPS):
            home = make_home(variant, f"q{rep}")
            inst = None
            try:
                inst, res = start_timing(variant, rep, home, f"{mode}-start")
                if inst.alive() and res.get("hotkey_ready_ms"):
                    hide(inst)
                    time.sleep(3)
                    if mode == "latency":
                        latency_block(inst, variant, rep)
                    else:
                        cycle_block_short(inst, variant, rep)
                    hide(inst)
            finally:
                if inst is not None:
                    inst.quit()
            shutil.rmtree(home, ignore_errors=True)


def cycle_block_short(inst, variant, rep):
    base = snapshot(inst, "before-show")
    t_show = time.time()
    hotkey_show()
    time.sleep(0.5)
    shown = snapshot(inst, "window-shown")
    dump(inst, variant, rep, "mem-window-shown")
    type_safely(inst, t_show, QUERY)
    time.sleep(0.8)
    typed = snapshot(inst, f"typed '{QUERY}'")
    hide(inst)
    time.sleep(1)
    h1 = snapshot(inst, "hidden+1s")
    time.sleep(10)
    h10 = snapshot(inst, "hidden+11s")
    log(kind="mem", variant=variant, rep=rep, snaps=[base, shown, typed, h1, h10])


def main():
    os.makedirs(OUT, exist_ok=True)
    if os.environ.get("PERF_MODE") == "gap":
        return gap_mode()
    if os.environ.get("PERF_MODE") in ("latency", "mem"):
        return quick_mode(os.environ["PERF_MODE"])
    rc, out = probe("trusted")
    log(kind="env", accessibility_trusted=out, bin=BIN, uptime=sh(["uptime"]).stdout.strip(),
        apps={v: count_apps(v) for v in VARIANTS})
    for variant in VARIANTS:
        for rep in range(REP_START, REP_START + REPS):
            home = make_home(variant, rep)
            inst = None
            try:
                inst, res = start_timing(variant, rep, home, "cold-no-cache")
                if inst.alive() and res.get("hotkey_ready_ms"):
                    log(kind="hide-after-first-window", variant=variant, rep=rep, hidden=hide(inst))
                    idle_block(inst, variant, rep, "after-first-start")
                    cycle_block(inst, variant, rep)
                    latency_block(inst, variant, rep)
                    hide(inst)
                    time.sleep(1)
                inst.quit()
                # second start: cache on disk now
                inst, _ = start_timing(variant, rep, home, "warm-cache")
                if inst.alive():
                    log(kind="hide-after-first-window", variant=variant, rep=rep, phase="warm", hidden=hide(inst))
                    time.sleep(10)
                    log(kind="warm-idle-snapshot", variant=variant, rep=rep, snap=snapshot(inst, "warm start +10s hidden"))
                inst.quit()
            finally:
                if inst is not None:
                    inst.quit()
            shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    main()

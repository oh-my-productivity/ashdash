#!/usr/bin/env python3
"""The static half of the testing policy (docs/TESTING.md): what a test that runs on a developer's machine must not do.

Only unit tests and integration tests run locally. End-to-end, GUI, VM, stress and performance work runs on CI. This checker reads
the sources and fails when code that runs on the local gate (scripts/check.sh) does something that belongs on CI:

  Rust   every test function that is not `#[ignore]`d (and the helper functions next to it) in crates/*/tests/**, in `#[cfg(test)]`
         modules and in test-only files (tests.rs, testperf.rs ...)
  shell  scripts/check.sh and every shell script it runs, directly or through another script (tests/hooks, tests/policy ...)

Rules (the name in brackets is what an opt-out names):

  [gui]      real GUI or OS interaction: osascript, screencapture, qlmanage, tart, `open` of an app, pasteboard access, AppKit/winit
             windows, synthetic key events, RealOs ...
  [sleep]    sleeps that add up to more than 200 ms in one Rust function (2 s in one script)
  [loop]     iteration over 1000 or more items (`for _ in 0..5000`), or a data set of 5000 or more items (`vec![x; 10_000]`)
  [network]  sockets to other machines, HTTP clients, curl, ssh, git push/fetch, cargo install ...
  [clock]    an assertion on elapsed wall-clock time (latency ceilings belong in the `perf_` tests that CI runs in release mode)
  [heavy]    shell only: release builds, benchmarks, packaging, the e2e and stress scripts, `cargo update`
  [ignore]   an `#[ignore]` without a reason of the form "ci: ..." (runs on CI) or "known: ..." (an open finding); cannot be opted out

An opt-out is a comment with a reason of at least ten characters, so a reviewer can judge it:

  // policy-allow: sleep -- the watcher needs 150 ms to settle; there is no event to wait for        (this line and the next code line)
  // policy-allow-fn: loop -- 2,000 rows is the smallest size that shows the bug                    (the whole function that follows)
  # policy-allow: network -- loopback only                                                           (shell: this line and the next)
  # policy-allow-file: gui -- this test only greps the text of the scripts it checks                (shell: the whole file)

Usage: policy_check.py [--root DIR] [--list-allowed] [--quiet]     exit status 1 when a rule is broken, 2 on a usage error.
"""
import argparse
import os
import re
import sys

# ---- thresholds (docs/TESTING.md quotes them; change both together) ----------------------------------------------------------------
RUST_SLEEP_LIMIT_MS = 200
SHELL_SLEEP_LIMIT_S = 2
LOOP_LIMIT = 1000      # iterations
SIZE_LIMIT = 5000      # items of a data set
STRING_LIMIT = 1_000_000  # characters of one generated string (`"x".repeat(n)`)
BYTES_LIMIT = 64 * 1024 * 1024  # bytes of a generated buffer (`vec![0u8; n]`)
MIN_REASON = 10        # characters

RULES = ("gui", "sleep", "loop", "network", "clock", "heavy")   # what an opt-out may name; "ignore" and "opt-out" findings cannot be silenced

ALLOW_RE = re.compile(r"policy-allow(-fn|-file)?:\s*([a-z]+(?:\s*,\s*[a-z]+)*)\s*(?:--|\u2014|\u2013)\s*(\S.*)")
ALLOW_BAD_RE = re.compile(r"policy-allow(?:-fn|-file)?\b")


class Finding:
    def __init__(self, path, line, rule, message):
        self.path, self.line, self.rule, self.message = path, line, rule, message

    def __str__(self):
        return f"{self.path}:{self.line}: [{self.rule}] {self.message}"


# ---- Rust source handling ----------------------------------------------------------------------------------------------------------
def rust_views(src):
    """Return (code, skeleton): `code` is the source with comments blanked, `skeleton` also blanks the inside of string and char
    literals. Both keep every character offset and every newline, so offsets and line numbers agree with the original."""
    n = len(src)
    code = list(src)
    skel = list(src)

    def blank(arr, a, b):
        for k in range(a, b):
            if arr[k] != "\n":
                arr[k] = " "

    i = 0
    while i < n:
        c = src[i]
        nxt = src[i + 1] if i + 1 < n else ""
        if c == "/" and nxt == "/":
            j = src.find("\n", i)
            j = n if j < 0 else j
            blank(code, i, j)
            blank(skel, i, j)
            i = j
        elif c == "/" and nxt == "*":
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth += 1
                    j += 2
                elif src.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            blank(code, i, j)
            blank(skel, i, j)
            i = j
        elif c == "r" and re.match(r'r#*"', src[i:i + 40]) and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            m = re.match(r'r(#*)"', src[i:])
            hashes = m.group(1)
            end = src.find('"' + hashes, i + len(m.group(0)))
            end = n if end < 0 else end + 1 + len(hashes)
            blank(skel, i + len(m.group(0)), max(i + len(m.group(0)), end - 1 - len(hashes)))
            i = end
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            blank(skel, i + 1, min(j, n))
            i = j + 1
        elif c == "'":
            # a char literal ('a', '\n', '\u{1F600}') or a lifetime ('a)
            m = re.match(r"'(?:\\u\{[0-9a-fA-F]+\}|\\x[0-9a-fA-F]{2}|\\.|[^\\'])'", src[i:i + 14])
            if m:
                blank(skel, i + 1, i + len(m.group(0)) - 1)
                i += len(m.group(0))
            else:
                i += 1
        else:
            i += 1
    return "".join(code), "".join(skel)


def match_brace(skel, open_idx):
    depth = 0
    for k in range(open_idx, len(skel)):
        ch = skel[k]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return k
    return len(skel) - 1


def line_of(src, idx):
    return src.count("\n", 0, idx) + 1


FN_RE = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")


def rust_functions(code, skel):
    """Yield dicts {name, start, body_start, body_end, attrs} for every function with a body."""
    out = []
    for m in FN_RE.finditer(skel):
        j = m.end()
        depth_p = 0
        k = j
        body = None
        while k < len(skel):
            ch = skel[k]
            if ch in "([":
                depth_p += 1
            elif ch in ")]":
                depth_p -= 1
            elif ch == ";" and depth_p <= 0:
                break
            elif ch == "{" and depth_p <= 0:
                body = k
                break
            k += 1
        if body is None:
            continue
        end = match_brace(skel, body)
        # attributes and doc comments directly above: walk back over attribute blocks, qualifiers and blank space
        head = m.start()
        pre = skel[:head]
        qual = re.search(r"((?:\b(?:pub(?:\([^)]*\))?|async|unsafe|const|extern(?:\s+\"[^\"]*\")?)\s+)*)$", pre)
        top = head - (len(qual.group(1)) if qual else 0)
        attrs = []
        pos = top
        while True:
            seg = skel[:pos].rstrip()
            if seg.endswith("]"):
                # find the matching '[' backwards, then the '#'
                depth, q = 0, len(seg) - 1
                while q >= 0:
                    if seg[q] == "]":
                        depth += 1
                    elif seg[q] == "[":
                        depth -= 1
                        if depth == 0:
                            break
                    q -= 1
                h = seg.rfind("#", 0, q + 1)
                if q < 0 or h < 0 or seg[h + 1:q].strip() not in ("", "!"):
                    break
                attrs.append(code[h:len(seg)])
                pos = h
            else:
                break
        out.append({"name": m.group(1), "start": top, "attr_top": pos, "body_start": body, "body_end": end, "attrs": attrs})
    return out


def is_test_attr(attrs):
    return any(re.search(r"#\[\s*(?:[A-Za-z_:]+::)?test\b", a) for a in attrs)


def is_ignored(attrs):
    return any(re.search(r"#\[\s*ignore\b", a) for a in attrs)


def cfg_test_regions(code, skel):
    """Byte ranges of `#[cfg(test)] mod name { ... }` blocks."""
    out = []
    for m in re.finditer(r"#\[\s*cfg\(\s*test\s*\)\s*\]", skel):
        j = m.end()
        mm = re.match(r"\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+[A-Za-z_][A-Za-z0-9_]*\s*\{", skel[j:])
        if mm:
            open_idx = j + mm.end() - 1
            out.append((open_idx, match_brace(skel, open_idx)))
    return out


def is_test_file(rel):
    parts = rel.split("/")
    base = parts[-1]
    if "tests" in parts[:-1] and parts[0] == "crates":
        return True
    return base in ("tests.rs", "testperf.rs") or base.endswith("_tests.rs") or base.startswith("test_")


# ---- allowances ----------------------------------------------------------------------------------------------------------------------
def collect_allowances(lines, comment_prefix, path, findings):
    """Return (line_allow: {line number: set(rules)}, fn_allow: [(line, set)], file_allow: set). Reports malformed opt-outs."""
    line_allow, fn_allow, file_allow = {}, [], set()
    for no, text in enumerate(lines, 1):
        if "policy-allow" not in text:
            continue
        # only comments count: the marker must come after a comment opener
        cpos = text.find(comment_prefix)
        if cpos < 0 or text.find("policy-allow") < cpos:
            continue
        m = ALLOW_RE.search(text)
        if not m:
            if ALLOW_BAD_RE.search(text):
                findings.append(Finding(path, no, "opt-out", "malformed policy-allow: write `policy-allow: <rule> -- <reason>`"))
            continue
        kind, rules, reason = m.group(1), {r.strip() for r in m.group(2).split(",")}, m.group(3).strip()
        bad = rules - set(RULES)
        if bad:
            findings.append(Finding(path, no, "opt-out", f"unknown rule {', '.join(sorted(bad))} (rules: {', '.join(RULES)})"))
            continue
        if len(reason) < MIN_REASON:
            findings.append(Finding(path, no, "opt-out", f"the reason after -- must say why (at least {MIN_REASON} characters)"))
            continue
        if kind == "-file":
            file_allow |= rules
        elif kind == "-fn":
            fn_allow.append((no, rules, reason))
        else:
            line_allow[no] = (rules, reason)
    return line_allow, fn_allow, file_allow


class Allowed:
    """Which findings an opt-out silenced, kept for --list-allowed."""
    used = []


def allowed_at(line_allow, lines, line, rule, comment_prefix):
    for ln in (line, line - 1):
        hit = line_allow.get(ln)
        if hit and rule in hit[0]:
            return hit[1]
    # a comment block directly above (consecutive comment lines) may carry the marker
    k = line - 1
    while k >= 1 and lines[k - 1].strip().startswith(comment_prefix):
        hit = line_allow.get(k)
        if hit and rule in hit[0]:
            return hit[1]
        k -= 1
    return None


# ---- Rust rules ----------------------------------------------------------------------------------------------------------------------
GUI_COMMANDS = ("osascript|screencapture|tart|qlmanage|open|killall|pkill|pbcopy|pbpaste|say|afplay|hdiutil|launchctl|pmset|tccutil|"
                "swiftc|xcodebuild|codesign|spctl|sudo|caffeinate|mdfind|defaults")
NET_COMMANDS = "curl|wget|ssh|scp|rsync|nc|ncat|telnet|ftp|sftp|ping|nslookup|dig|gh"
RUST_GUI_RE = re.compile(
    r'(?:Command::new\(\s*"(?:/[\w./-]*/)?(?:%s)")|'
    r"\b(?:SystemPasteboard|NSWindow|NSPanel|NSApplication|NSPasteboard|NSWorkspace|NSAlert|NSStatusBar|NSStatusItem|"
    r"EventLoopBuilder|EventLoop::new|eframe::run_native|CGEventPost|CGEventCreateKeyboardEvent|CGEventCreateMouseEvent|"
    r"AXUIElementCreate\w*|CGWindowListCreateImage|generalPasteboard|clearContents|writeObjects|setString_forType|"
    r"set_clipboard|SMAppService|LSOpenCFURLRef|CGDisplayCreateImage|capture_selection|DialogPrompter|GlobalHotKeyManager)\b|"
    r"\b(?:engine::notify|opendocs::notify|Pane::\w+\.open)\(" % GUI_COMMANDS
)
# `RealOs` runs whatever command it is given (tests use it with `sleep`, `echo`, `yes`); it is a problem with a tool of the machine's GUI/OS.
RUST_REALOS_TOOL_RE = re.compile(r'\bCmd::new\(\s*"(?:%s)"|\bCmd::osascript\(' % GUI_COMMANDS)
RUST_NET_RE = re.compile(
    r'\b(?:TcpStream::connect|UdpSocket::bind|reqwest::|ureq::|hyper::|isahc::|attohttpc::|surf::|minreq::)|'
    r'Command::new\(\s*"(?:/[\w./-]*/)?(?:%s)"' % NET_COMMANDS
)
RUST_SLEEP_RE = re.compile(
    r"\bsleep\(\s*(?:std::time::|core::time::)?Duration::from_(millis|secs|micros|nanos|secs_f64|secs_f32|minutes)\(\s*([0-9][0-9_]*(?:\.[0-9]+)?)"
)
RUST_FOR_RANGE_RE = re.compile(r"\bfor\b[^\n{;]*?\bin\s+\(?\s*(-?[0-9][0-9_]*)(?:[a-z0-9]*)\s*\.\.(=?)\s*(-?[0-9][0-9_]*)")
RUST_ITER_RANGE_RE = re.compile(r"\(\s*(-?[0-9][0-9_]*)(?:[a-z0-9]*)\s*\.\.(=?)\s*(-?[0-9][0-9_]*)(?:[a-z0-9]*)\s*\)\s*\.")
# (regex, limit): the first group is an integer literal or a product of them (`100 * 1024 * 1024`)
INT_EXPR = r"([0-9][0-9_]*(?:\s*\*\s*[0-9][0-9_]*)*)"
RUST_SIZE_RES = (
    (re.compile(r"\bvec!\s*\[\s*(?:0|[^;\]\n]*(?:u8|b'))\s*;\s*" + INT_EXPR), BYTES_LIMIT),
    (re.compile(r"\bvec!\s*\[[^;\]\n]*;\s*" + INT_EXPR), SIZE_LIMIT),
    (re.compile(r"\.repeat\(\s*" + INT_EXPR + r"\s*\)"), STRING_LIMIT),
    (re.compile(r"\brepeat(?:_n)?\([^;\n]*?\)\s*\.take\(\s*" + INT_EXPR + r"\s*\)"), SIZE_LIMIT),
    (re.compile(r"\.take\(\s*" + INT_EXPR + r"\s*\)"), SIZE_LIMIT),
    (re.compile(r"\bVec::from_elem\([^;\n]*?,\s*" + INT_EXPR + r"\s*\)"), SIZE_LIMIT),
)
RUST_ASSERT_RE = re.compile(r"\bassert(?:_[a-z]+)?!\s*\(")
RUST_CLOCK_HINT_RE = re.compile(r"\.elapsed\(\)|\bInstant::now\(\)|\b(?:elapsed|took|latency|p50|p90|p95|p99|per_call|per_key)\b")
HANG_GUARD_MS = 5000   # an upper bound of at least this only catches a hang (10x or more over normal), it does not time the code
RUST_DURATION_LIT_RE = re.compile(r"from_(millis|secs|secs_f64|secs_f32|micros)\(\s*([0-9][0-9_]*(?:\.[0-9]+)?)")
RUST_CLOCK_CMP_RE = re.compile(r"(?<![<>=-])(?:<=|>=|<|>)(?![<>=])")


def num(text):
    return float(text.replace("_", ""))


def product(text):
    out = 1.0
    for part in text.split("*"):
        out *= num(part.strip())
    return out


def check_rust_function(path, src, code, skel, fn, findings, line_allow, fn_rules, lines):
    body = code[fn["body_start"]:fn["body_end"] + 1]
    sbody = skel[fn["body_start"]:fn["body_end"] + 1]
    base = fn["body_start"]

    def report(idx, rule, message):
        if rule in fn_rules:
            Allowed.used.append((path, line_of(src, base + idx), rule, fn_rules[rule]))
            return
        line = line_of(src, base + idx)
        why = allowed_at(line_allow, lines, line, rule, "//")
        if why:
            Allowed.used.append((path, line, rule, why))
            return
        findings.append(Finding(path, line, rule, message))

    for m in RUST_GUI_RE.finditer(body):
        if sbody[m.start():m.end()].isspace():
            continue
        report(m.start(), "gui", f"`{m.group(0).strip()}` touches the real OS or GUI; a local test uses a fake, the real thing is `#[ignore = \"ci: ...\"]`")
    if re.search(r"\bRealOs\b", body):
        for m in RUST_REALOS_TOOL_RE.finditer(body):
            report(m.start(), "gui", f"`{m.group(0).strip()}` run through `RealOs` touches the real OS or GUI; use a fake or make it `#[ignore = \"ci: ...\"]`")
    for m in RUST_NET_RE.finditer(body):
        report(m.start(), "network", f"`{m.group(0).strip()}` uses the network; tests use no network (loopback sockets and unix sockets are fine)")

    total_ms, first = 0.0, None
    for m in RUST_SLEEP_RE.finditer(body):
        unit, value = m.group(1), num(m.group(2))
        ms = {"millis": 1, "secs": 1000, "micros": 0.001, "nanos": 0.000001, "secs_f64": 1000, "secs_f32": 1000, "minutes": 60000}[unit] * value
        total_ms += ms
        first = m.start() if first is None else first
    if total_ms > RUST_SLEEP_LIMIT_MS:
        report(first, "sleep", f"sleeps add up to {total_ms:.0f} ms (limit {RUST_SLEEP_LIMIT_MS} ms): wait for the condition, or make it `#[ignore = \"ci: ...\"]`")

    for m in RUST_FOR_RANGE_RE.finditer(sbody if False else body):
        lo, incl, hi = num(m.group(1)), m.group(2) == "=", num(m.group(3))
        count = hi - lo + (1 if incl else 0)
        if count >= LOOP_LIMIT:
            report(m.start(), "loop", f"a loop over {count:.0f} items (limit {LOOP_LIMIT - 1}): use a smaller case here and the big one as `#[ignore = \"ci: ...\"]`")
    for m in RUST_ITER_RANGE_RE.finditer(body):
        lo, incl, hi = num(m.group(1)), m.group(2) == "=", num(m.group(3))
        count = hi - lo + (1 if incl else 0)
        if count >= LOOP_LIMIT:
            report(m.start(), "loop", f"an iteration over {count:.0f} items (limit {LOOP_LIMIT - 1}): use a smaller case here and the big one as `#[ignore = \"ci: ...\"]`")
    byte_buffers = set()   # `vec![0u8; n]` starts: judged by the byte limit only, not again as an item count
    for rx, limit in RUST_SIZE_RES:
        for m in rx.finditer(body):
            if rx is RUST_SIZE_RES[1][0] and m.start() in byte_buffers:
                continue
            if rx is RUST_SIZE_RES[0][0]:
                byte_buffers.add(m.start())
            size = product(m.group(1))
            if size >= limit:
                unit = "bytes" if rx is RUST_SIZE_RES[0][0] else "characters" if rx is RUST_SIZE_RES[2][0] else "items"
                report(m.start(), "loop", f"a data set of {size:.0f} {unit} (limit {limit - 1}): use a smaller case here and the big one as `#[ignore = \"ci: ...\"]`")

    for m in RUST_ASSERT_RE.finditer(sbody):
        open_idx = m.end() - 1
        depth, k = 0, open_idx
        while k < len(sbody):
            if sbody[k] == "(":
                depth += 1
            elif sbody[k] == ")":
                depth -= 1
                if depth == 0:
                    break
            k += 1
        text = body[open_idx:k + 1]
        stext = sbody[open_idx:k + 1]
        lits = [num(v) * {"millis": 1, "secs": 1000, "secs_f64": 1000, "secs_f32": 1000, "micros": 0.001}[u] for u, v in RUST_DURATION_LIT_RE.findall(text)]
        if lits and max(lits) >= HANG_GUARD_MS:
            continue
        if RUST_CLOCK_HINT_RE.search(stext) and RUST_CLOCK_CMP_RE.search(stext) and re.search(
            r"Duration::|\bfrom_(?:millis|secs)|as_millis|as_secs|as_micros|\bms\b|_ms\b", text
        ):
            report(m.start(), "clock", "an assertion on elapsed wall-clock time; machine speed decides it. Count work or compare scaling (docs/TESTING.md) or make it a `perf_` test")


def check_rust_file(root, rel, findings):
    path = os.path.join(root, rel)
    with open(path, encoding="utf-8", errors="replace") as f:
        src = f.read()
    lines = src.split("\n")
    code, skel = rust_views(src)
    line_allow, fn_allow_marks, file_allow = collect_allowances(lines, "//", rel, findings)
    whole_file = is_test_file(rel)
    regions = [] if whole_file else cfg_test_regions(code, skel)
    if not whole_file and not regions:
        return 0
    fns = rust_functions(code, skel)
    for fn in fns:
        if not (whole_file or any(a <= fn["body_start"] <= b for a, b in regions)):
            continue
        for attr in fn["attrs"]:
            if re.match(r"#\[\s*ignore\b", attr) and not re.match(r'#\[\s*ignore\s*=\s*"(?:ci|known): \S', attr):
                findings.append(Finding(rel, line_of(src, code.find(attr, fn["attr_top"] - 1)) if attr in code else 1, "ignore",
                                        "an ignored test needs a reason: #[ignore = \"ci: <why it cannot run locally>\"] (or \"known: <open finding>\")"))
    candidates = []
    for fn in fns:
        in_region = whole_file or any(a <= fn["body_start"] <= b for a, b in regions)
        if in_region and not is_ignored(fn["attrs"]):
            candidates.append(fn)
    # A helper that only `#[ignore]`d tests call is CI code too: check the tests and what the tests reach (by name, in this file).
    by_name = {}
    for fn in fns:
        by_name.setdefault(fn["name"], []).append(fn)
    has_tests = any(is_test_attr(fn["attrs"]) for fn in fns if whole_file or any(a <= fn["body_start"] <= b for a, b in regions))
    if has_tests:
        reach, todo = set(), [fn for fn in candidates if is_test_attr(fn["attrs"])]
        while todo:
            fn = todo.pop()
            if fn["body_start"] in reach:
                continue
            reach.add(fn["body_start"])
            body = skel[fn["body_start"]:fn["body_end"] + 1]
            for name in set(re.findall(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*(?:::<[^>]*>)?\s*\(", body)):
                for callee in by_name.get(name, []):
                    if not is_ignored(callee["attrs"]):
                        todo.append(callee)
        candidates = [fn for fn in candidates if fn["body_start"] in reach]
    checked = 0
    seen = set()
    for fn in candidates:
        if fn["body_start"] in seen:
            continue
        seen.add(fn["body_start"])
        # an opt-out for the whole function sits in the comment block just above its attributes
        fn_rules = {}
        decl_line = line_of(src, fn["attr_top"])
        k = decl_line - 1
        while k >= 1 and (lines[k - 1].strip().startswith("//") or lines[k - 1].strip().startswith("#[") or lines[k - 1].strip() == ""):
            if lines[k - 1].strip() == "":
                break
            for no, rules, reason in fn_allow_marks:
                if no == k:
                    for r in rules:
                        fn_rules[r] = reason
            k -= 1
        for r in file_allow:
            fn_rules.setdefault(r, "file-wide opt-out")
        check_rust_function(rel, src, code, skel, fn, findings, dict(line_allow), fn_rules, lines)
        checked += 1
    return checked


# ---- shell rules ---------------------------------------------------------------------------------------------------------------------
SH_CMD = r"(?:^|[;&|(`]|\$\(|\bthen\b|\bdo\b|\bsudo\b|\bxargs\b|\benv\b)\s*"
SH_GUI_RE = re.compile(
    r"(?:" + SH_CMD + r"(?:/[\w./-]*/)?(?:osascript|screencapture|qlmanage|tart|pbcopy|pbpaste|tccutil|launchctl|swiftc|xcodebuild|hdiutil|afplay|"
    r"killall|pkill|caffeinate|say|open|xdg-open|lipo|codesign|spctl)\b(?!\s*=)|\bgui-exec\b|\bosascript\b|\bscreencapture\b|\bqlmanage\b)"
)
SH_NET_RE = re.compile(
    SH_CMD + r"(?:curl|wget|ssh|scp|rsync|nc|ncat|telnet|ftp|sftp|ping|nslookup|dig|gh|brew|npm|npx|pip3?|pipx|cargo\s+(?:install|publish|login|search|owner|yank)|"
    r"git\s+(?:push|fetch|pull|clone|ls-remote|remote\s+update|submodule\s+update))\b"
)
SH_HEAVY_RE = re.compile(
    r"\bcargo\s+(?:bench|update)\b|\bcargo\s+[a-z]+\b[^#\n]*--release\b|\bcargo\s+test\b[^#\n]*--ignored\b|\bcargo\s+test\b[^#\n]*--include-ignored\b|"
    r"packaging/(?:build-app|make-dmg|selftest)\.sh|\btests/e2e/|\bscripts/qa/|\bscripts/ci/|\bscripts/ci-run\.sh|\bstress-timing\b|\bperf-gui\b|"
    r"\bcargo\s+deny\b|\bcargo\s+fuzz\b|\bproptest\b.*PROPTEST_CASES=[0-9]{4,}"
)
SH_SLEEP_RE = re.compile(r"(?:^|[;&|(]|\bdo\b|\bthen\b)\s*sleep\s+([0-9]+(?:\.[0-9]+)?)([smhd]?)\b")
SH_LOOP_RES = (
    re.compile(r"\bseq\s+(?:[0-9]+\s+)?([0-9]{4,})\b"),
    re.compile(r"\{\s*[0-9]+\.\.([0-9]{4,})\s*\}"),
    re.compile(r"\(\(\s*[a-z_]+\s*=\s*[0-9]+\s*;\s*[a-z_]+\s*<=?\s*([0-9]{4,})\s*;"),
)


def check_shell_file(root, rel, findings, mark_files):
    path = os.path.join(root, rel)
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.read().split("\n")
    line_allow, _, file_allow = collect_allowances(lines, "#", rel, findings)
    total_sleep = 0.0
    first_sleep = None
    for no, text in enumerate(lines, 1):
        stripped = text.strip()
        if stripped.startswith("#") or not stripped:
            continue
        if re.match(r"(?:echo|printf|say|warn|die|fail|bad|ok|note|log|usage)\b", stripped):
            continue
        text = re.sub(r"\b(echo|printf)\s+(\"(?:[^\"\\]|\\.)*\"|'[^']*')", r"\1", text)
        body = re.sub(r"\s#\s.*$", "", text) if text.count('"') % 2 == 0 and text.count("'") % 2 == 0 else text

        def report(rule, message):
            if rule in file_allow:
                Allowed.used.append((rel, no, rule, "file-wide opt-out"))
                return
            why = allowed_at(line_allow, lines, no, rule, "#")
            if why:
                Allowed.used.append((rel, no, rule, why))
                return
            findings.append(Finding(rel, no, rule, message))

        m = SH_GUI_RE.search(body)
        if m:
            report("gui", f"`{m.group(0).strip(' ;&|(`')}`: real GUI/OS interaction does not run on the local gate; it runs on CI (tests/e2e)")
        m = SH_NET_RE.search(body)
        if m:
            report("network", f"`{m.group(0).strip(' ;&|(`')}` needs the network; the local gate uses none (only crates.io for dependencies)")
        m = SH_HEAVY_RE.search(body)
        if m:
            report("heavy", f"`{m.group(0).strip()}` is CI work (release builds, benchmarks, packaging, e2e and stress scripts)")
        for m in SH_SLEEP_RE.finditer(body):
            secs = float(m.group(1)) * {"": 1, "s": 1, "m": 60, "h": 3600, "d": 86400}[m.group(2)]
            if "sleep" not in file_allow and allowed_at(line_allow, lines, no, "sleep", "#") is None:
                total_sleep += secs
                first_sleep = first_sleep or no
        for rx in SH_LOOP_RES:
            m = rx.search(body)
            if m and int(m.group(1)) >= LOOP_LIMIT:
                report("loop", f"a loop of {m.group(1)} iterations (limit {LOOP_LIMIT - 1})")
    if total_sleep > SHELL_SLEEP_LIMIT_S:
        findings.append(Finding(rel, first_sleep, "sleep", f"sleeps add up to {total_sleep:g} s (limit {SHELL_SLEEP_LIMIT_S} s)"))
    return 1


REPO_PATH_RE = re.compile(r"(?<![\w.-])((?:\.githooks|scripts|tests)/[\w./+-]+)")


def is_shell_script(path):
    """A shell script by extension or shebang. Python helpers are not scanned: they are libraries of the gate, not tests."""
    if path.endswith(".sh"):
        return True
    if path.endswith((".py", ".md", ".rs", ".json", ".toml", ".yml", ".tsv")):
        return False
    try:
        with open(path, "rb") as f:
            head = f.readline(80)
    except OSError:
        return False
    return bool(re.match(rb"#!.*\b(?:ba|z|da)?sh\b", head))


def local_gate_scripts(root):
    """check.sh and, transitively, the repository scripts it runs (by path, by `$here/name`, or by a file name in its own folder)."""
    start = "scripts/check.sh"
    if not os.path.exists(os.path.join(root, start)):
        return []
    seen, todo = [], [start]
    while todo:
        rel = todo.pop()
        if rel in seen or not os.path.isfile(os.path.join(root, rel)):
            continue
        seen.append(rel)
        with open(os.path.join(root, rel), encoding="utf-8", errors="replace") as f:
            raw = f.read()
        if re.search(r"^\s*#\s*policy-allow-file:", raw, re.M):
            continue   # a file that opted out as a whole (its text is test data or a tool for CI) does not pull in what it names
        if True:
            # only what runs counts: not comments, not the text of messages
            text = "\n".join(
                ln for ln in raw.split("\n")
                if not ln.strip().startswith("#") and not re.match(r"\s*(?:echo|printf|say|warn|die|fail|bad|ok|note|log|usage)\b", ln)
            )
        folder = os.path.dirname(rel)
        refs = set(REPO_PATH_RE.findall(text))
        siblings = os.listdir(os.path.join(root, folder)) if folder else []
        if os.path.basename(rel) == "run-tests.sh":
            # convention: run-tests.sh runs the *.test.sh files next to it (usually by a loop variable, which no text search can follow)
            refs |= {os.path.join(folder, n) for n in siblings if n.endswith(".test.sh")}
        for name in siblings:
            full = os.path.join(folder, name)
            if os.path.isfile(os.path.join(root, full)) and name != os.path.basename(rel) and re.search(r"(?<![\w.-])" + re.escape(name) + r"(?![\w-])", text):
                refs.add(full)
        for r in refs:
            r = r.rstrip(".,;:)\"'")
            if os.path.isdir(os.path.join(root, r)):
                continue
            full = os.path.join(root, r)
            if os.path.isfile(full) and is_shell_script(full):
                todo.append(r)
    return sorted(seen)


def rust_files(root):
    out = []
    for base in ("crates",):
        for dirpath, dirnames, filenames in os.walk(os.path.join(root, base)):
            dirnames[:] = [d for d in dirnames if d not in ("target", "benches", "examples", ".git")]
            for name in filenames:
                if name.endswith(".rs"):
                    out.append(os.path.relpath(os.path.join(dirpath, name), root).replace(os.sep, "/"))
    return sorted(out)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    ap.add_argument("--list-allowed", action="store_true", help="print the opt-outs that silenced a finding")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()
    root = os.path.abspath(args.root)
    if not os.path.isdir(root):
        print(f"policy-check: no such directory {root}", file=sys.stderr)
        return 2

    findings = []
    n_fn = 0
    for rel in rust_files(root):
        n_fn += check_rust_file(root, rel, findings)
    scripts = local_gate_scripts(root)
    for rel in scripts:
        check_shell_file(root, rel, findings, scripts)

    for f in sorted(findings, key=lambda f: (f.path, f.line)):
        print(f, file=sys.stderr)
    if args.list_allowed:
        for path, line, rule, why in sorted(set(Allowed.used)):
            print(f"allowed {path}:{line}: [{rule}] {why}")
    if findings:
        print(
            f"policy-check: {len(findings)} problem(s). Only unit and integration tests run locally; the rest runs on CI. Mark the test "
            f"`#[ignore = \"ci: <reason>\"]` and keep a small variant, or fix it. See docs/TESTING.md.",
            file=sys.stderr,
        )
        return 1
    if not args.quiet:
        print(f"policy-check: ok ({n_fn} Rust functions in test code, {len(scripts)} local-gate scripts; {len(set(Allowed.used))} documented opt-outs)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

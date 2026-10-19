#!/usr/bin/env python3
"""Merge QA result files (id <TAB> status <TAB> evidence; later files override earlier ones) into the markdown tables of
a QA run document.   usage: report.py out-table.md results1.tsv [results2.tsv ...]
The prose of the run document (summary, bug list, owner steps) is written by hand around the generated tables."""
import re
import sys

ITEMS = {
    # packaged install
    "PKG-01": "Fresh clone: `ARCHS=native packaging/build-app.sh` + `make-dmg.sh`",
    "PKG-02": "Gatekeeper switched on in the clone",
    "PKG-03": "DMG quarantined like a browser download, mounted",
    "PKG-04": "Ashdash.app copied to /Applications",
    "PKG-05": "App quarantined (`0083;...;Safari`), `spctl --assess`",
    "PKG-06": "Double-click equivalent (`open`) of the quarantined ad-hoc app",
    "PKG-07": "Buttons of the first dialog",
    "PKG-08": "'Done' leaves the app not running",
    "PKG-09": "Finder File > Open (= right-click > Open) on macOS 15",
    "PKG-10": "System Settings > Privacy & Security lists the blocked app",
    "PKG-11": "'Open Anyway' in Privacy & Security",
    "PKG-12": "Second dialog after 'Open Anyway'",
    "PKG-13": "App runs after the second dialog",
    "PKG-14": "`xattr -dr com.apple.quarantine`, then open",
    "PKG-15": "First launch on the fresh account: prompts, windows, data folder",
    "PKG-16": "First hotkey press",
    "PKG-17": "Login item from the bundle (`ashdash login-item`)",
    "PKG-18": "`ashdash://prefs` from the installed bundle",
    "PKG-19": "First `> command`: Automation (Terminal) prompt, Don't Allow",
    "PKG-20": "Same after `tccutil reset`, Allow",
    "PKG-21": "Auto-paste without Accessibility on a fresh account",
    # 3.1
    "3.1.1a": "Hotkey/show to visible window", "3.1.1b": "Input focused after show", "3.1.1c": "Caret blinks",
    "3.1.2": "20 quick toggles: no stuck state, one window", "3.1.3a": "Shows over a full-screen app",
    "3.1.3b": "Multi-monitor / other Space", "3.1.4": "Focus returns to the previous app on hide",
    "3.1.5a": "Second instance", "3.1.5b": "Hotkey already taken by another process",
    "3.1.6a": "Hotkey changed in config: live, old key dead", "3.1.6b": "Invalid hotkey keeps the old one",
    "3.1.7": "No Dock icon, no Cmd+Tab entry",
    # 3.2
    "3.2.1a": "`saf` lists Safari first", "3.2.1b": "`vsc` / `visual studio`", "3.2.2a": "`2+3*4` = 14", "3.2.2b": "`= sqrt(81)` = 9",
    "3.2.2c": "`1/0` and `((((` do not crash", "3.2.3a": "100 KB paste", "3.2.3b": "5,000 `(` paste", "3.2.4a": "Unicode: ü, CJK, emoji",
    "3.2.4b": "IME composition", "3.2.5a": "Backspace to empty clears results", "3.2.5b": "Cmd+Backspace", "3.2.5c": "Cmd+A, type",
    "3.2.6": "Fast typing: list belongs to the final text", "3.2.7a": "`g rust`", "3.2.7b": "`wiki x`", "3.2.7c": "Fallback searches for unknown text",
    "3.2.8a": "`~/Doc` path browsing", "3.2.8b": "`> echo hi` offers Terminal", "3.2.8c": "`open <name>` Spotlight files",
    "3.2.9a": "`shutdown` lists Shut Down", "3.2.9b": "Return on Shut Down asks first", "3.2.9c": "Esc declines, guest still up",
    "3.2.9d": "`lock` / `sleep` / `restart` / `logout`",
    # 3.3
    "3.3.1a": "Up/Down then Return", "3.3.1b": "Ctrl+N / Ctrl+P", "3.3.1c": "Up on the first row (wrap or stop)", "3.3.1d": "Selection scrolls into view",
    "3.3.2": "Return runs and hides", "3.3.3a": "Tab completes a keyword row", "3.3.3b": "Tab on an application row", "3.3.4": "One Esc closes, with text", "3.3.4b": "Next show starts clean",
    "3.3.5": "Cmd+1..9 and the row hints", "3.3.6a": "Cmd+L Large Type", "3.3.6b": "Esc dismisses Large Type",
    "3.3.7a": "Held Return on a confirmation never confirms", "3.3.7b": "Fresh Return confirms (Empty Trash through Finder)",
    "3.3.8": "Modifier overrides on a Script Filter item", "3.3.6c": "Large Type taller than the screen scrolls", "3.3.10": "Held modifier swaps the subtitle", "3.3.11": "Script Filter running row and rerun",
    "3.2.4c": "CJK glyphs are drawn", "3.6.8": "Launcher import prompt (Right Arrow > Install Workflow...)", "3.9.6": "Quick Look above the launcher", "3.3.9a": "Cmd+W / Cmd+H", "3.3.9b": "Cmd+Q",
    # 3.4
    "3.4.1": "Activating another app hides the launcher", "3.4.2": "Cmd+Tab hides it, no focus steal", "3.4.3": "Switching Spaces",
    "3.4.4a": "Display sleep / wake", "3.4.4b": "Screen lock, display hot-plug", "3.4.4c": "Real system sleep",
    # 3.5
    "3.5.1": "`clip`: newest first, duplicates move up", "3.5.2a": "Return copies an entry back", "3.5.2b": "Cmd+V pastes it (real TextEdit)",
    "3.5.3": "Concealed / transient pasteboard items never recorded", "3.5.4": "clipboard.json / usage.json are 0600",
    "3.5.5a": "File copy shown by name", "3.5.5b": "File copy restored as a file", "3.5.6": "Per-app ignore list, no restart",
    "3.5.7a": "`snip` lists snippets", "3.5.7b": "`{clipboard}` expansion", "3.5.8a": "Auto-paste without Accessibility: explains",
    "3.5.8b": "Auto-paste with Accessibility: real Cmd+V", "3.5.9": "Snippet auto-paste is not in clipboard history",
    "3.5.10": "Clip entry auto-paste", "3.5.11": "Real password manager",
    # 3.6
    "3.6.1a": "`ashdash import` without a terminal and without `--yes`", "3.6.1b": "`--yes` installs, keyword works at once",
    "3.6.2": "Package with a `../` entry", "3.6.3a": "Script Filter garbage output", "3.6.3b": "Script Filter that hangs (10 s limit)",
    "3.6.4a": "Disabled workflow loses its keywords", "3.6.4b": "`ashdash trigger`", "3.6.5a": "`workflow_dirs` edit adds a folder live",
    "3.6.5b": "Removing the entry removes it", "3.6.6a": "Finder route: import dialog", "3.6.6b": "Nothing installed without confirming",
    "3.6.7": "Other import-prompt checks of 3.6",
    # 3.7
    "3.7.1": "Notifications (workflow Post Notification)", "3.7.2a": "Automation prompt (Terminal) and denial", "3.7.2b": "Denial message names where to fix it",
    "3.7.2c": "Allow runs the command in Terminal", "3.7.2d": "System Events automation", "3.7.3a": "Safari bookmarks without Full Disk Access",
    "3.7.3b": "Safari bookmarks with Full Disk Access", "3.7.4": "No Contacts / Calendar / Photos prompts",
    # 3.8
    "3.8.1a": "Light/dark switch while open", "3.8.1b": "Custom theme applies live", "3.8.1c": "Broken theme refused", "3.8.2": "Retina, window on screen",
    "3.8.2b": "Non-Retina, text scaling", "3.8.3": "Idle CPU 60 s", "3.8.4": "kill -9 during clipboard writes, relaunch", "3.8.5": "Corrupt config.json",
    "3.8.6": "No network",
    # 3.9
    "3.9.0": "Path lists the file", "3.9.1a": "Right Arrow opens 'Actions for ...'", "3.9.1b": "Typing filters, Esc goes back", "3.9.2": "Open With...",
    "3.9.2b": "Open With runs the chosen app", "3.9.3a": "Option+Up adds to the buffer", "3.9.3b": "Right with a buffer opens buffer actions",
    "3.9.3c": "Hide/show empties the buffer", "3.9.4a": "Move to Trash asks, names the file", "3.9.4b": "Second fresh Return trashes it",
    "3.9.5": "No actions for system commands",
    # 3.11
    "3.11.1": "Idle memory / CPU of the real process", "3.11.2": "Process start to first hotkey show", "3.11.3": "`mdfind` file search timing",
    "3.11.4": "Browser-downloaded DMG on a Mac that never saw the project",
    # Wave 4/5
    "W5.1": "Highlight follows the result id when a rerun reorders the list", "W5.2a": "Slow Script Filter: running row then final list",
    "W5.2b": "Return runs the row that is on screen after the slow answer", "W5.2c": "Return in the frame a list is replaced",
    "W5.3a": "Crash note once in the footer", "W5.3b": "`ashdash paths` names the report", "W5.4a": "`ashdash://search` only fills the field",
    "W5.4b": "URL consent dialog, one per bundle at a time", "W5.4c": "Consent timeout (60 s) stores nothing", "W5.4d": "Asks again after a timeout",
    "W5.4e": "Allow runs the trigger and is stored", "W5.4f": "Stored answer: no dialog", "W5.4g": "Don't Allow is stored",
    "W5.5": "Transient workflow copy not in history, plain copy is",
    "KU-AS": "AppleScript `search`", "KU-JXA1": "JXA, bundle id form", "KU-JXA2": "JXA, path form", "KU-STATUS": "Menu-bar item: Preferences...",
    "KU-HELD": "Held Return in the import prompt never installs", "KU-HELD2": "Fresh Return installs", "SOAK": "2-hour soak", "SOAK-PM": "pmset sleepnow / displaysleepnow in the VM", "SETUP": "Bundle installed, app running from /Applications",
    "REPRO-PLACEHOLDER": "Wave 5 screenshot: placeholder shown while results for \"saf\" are listed",
}
SECTIONS = [
    ("Setup", r"SETUP"), ("Packaged install on a fresh account (`tests/e2e/qa/pkg-install.sh`)", r"PKG-"),
    ("3.1 Hotkey and window", r"3\.1\."), ("3.2 Typing and results", r"3\.2\."), ("3.3 Keyboard", r"3\.3\."), ("3.4 Focus loss", r"3\.4\."),
    ("3.5 Clipboard history and snippets", r"3\.5\."), ("3.6 Workflows", r"3\.6\."), ("3.7 Permissions", r"3\.7\."),
    ("3.8 Appearance and robustness", r"3\.8\."), ("3.9 Universal Actions panel and buffer", r"3\.9\."),
    ("3.11 Measurements that need the real process", r"3\.11\."), ("Wave 4/5 behaviours", r"W5\."),
    ("Other items from STATUS.md 'Known unverified items'", r"KU-"), ("Soak", r"SOAK"), ("Wave 5 screenshot investigation", r"REPRO-"),
]


def key(i):
    return [int(p) if p.isdigit() else p for p in re.split(r"(\d+)", i)]


def clean(t, n=420):
    t = re.sub(r"\s+", " ", t).strip().replace("|", "\\|")
    return t if len(t) <= n else t[: n - 1] + "…"


def main():
    out, files = sys.argv[1], sys.argv[2:]
    res = {}
    for f in files:
        for line in open(f, encoding="utf-8", errors="replace"):
            p = line.rstrip("\n").split("\t", 2)
            if len(p) == 3 and p[1] in ("PASS", "FAIL", "BLOCKED", "NEEDS-OWNER"):
                res[p[0]] = (p[1], p[2])
    counts = {}
    md = []
    for title, pat in SECTIONS:
        ids = sorted([i for i in res if re.match(pat, i)], key=key)
        if not ids:
            continue
        md += [f"### {title}", "", "| Item | Result | What was checked | Evidence |", "|---|---|---|---|"]
        for i in ids:
            st, ev = res[i]
            counts[st] = counts.get(st, 0) + 1
            md.append(f"| {i} | {st} | {ITEMS.get(i, '')} | {clean(ev)} |")
        md.append("")
    with open(out, "w", encoding="utf-8") as fh:
        fh.write("\n".join(md))
    print(counts, sum(counts.values()))


main()

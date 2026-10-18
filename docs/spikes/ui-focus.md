# Spike: does the egui window take key focus and give it back?

Question: when the global hotkey shows the launcher, does the eframe/winit window reliably become the
key window, and does hiding it return focus to the app that was frontmost before? If flaky, an NSPanel (non-activating,
floating, can become key) would be the alternative.

## Method

`docs/spikes/ui-focus.sh` (committed, re-runnable). Per iteration it activates one of Finder, Calculator, Terminal,
Safari, System Settings, records the frontmost app (via `NSWorkspace`, because System Events cannot see accessory apps),
presses the hotkey, types `zqN`, checks the frontmost app again, hides the launcher, and checks the frontmost app a
third time. Focus and typed text are read from the launcher's own `ASHDASH_TRACE=1` log (`focus=Some(true) +Nms after show`,
`query="zqN"`). The launcher runs with a throw-away `HOME` and a non-default hotkey (Ctrl+Opt+Shift+J), as a copy of the
debug binary under a unique name.

Machine: macOS, debug build, two runs of 10 iterations:

| Run | Hide with | Delay hotkey -> first key | Focus gained | Text arrived intact | Previous app frontmost again |
|-----|-----------|---------------------------|--------------|---------------------|------------------------------|
| A   | Esc, Esc  | 0.8 s                     | 10/10        | 9/10 (see below)    | 9/10                         |
| B   | hotkey    | 0.05 s (typing race)      | 10/10        | 10/10               | 9/10                         |

* Time from `show` to the window reporting `focused=true`: 13-30 ms in all 20 runs.
* With a 50 ms delay between hotkey and typing, no characters were lost (the first character sometimes arrives as its own
  `query="z"` event followed by `"zq9"`, which is just two key events in separate frames).
* Run A, iteration 4: the query was `Xzq4`: a stray `X` typed by something else on the machine arrived while the launcher was
  key. That is proof the launcher had focus, not a failure of it.

## The three "failures" are not the launcher

In run A/3, run B/4 and run B/9 the frontmost app afterwards was **TextEdit**, which was never part of the script. A control
loop that only activates Safari (no Ashdash process running) shows the same behaviour:

```
Safari / TextEdit / Safari      (activate Safari, wait 2 s, read frontmost; three times)
```

Something on this machine (a TextEdit instance, running as pid 86374 at the time) takes the foreground by itself every so
often. Excluding those, focus return was 20/20: `hide()` does `ViewportCommand::Visible(false)` followed by
`NSApplication.hide(nil)`, and macOS gives key status back to the previously active application.

## Findings

1. winit's window takes key focus reliably on `Visible(true)` + `Focus` because the process uses
   `ActivationPolicy::Accessory`; `NSApplication.activate` is performed by winit as part of `Focus`.
2. Return of focus on hide is reliable because we hide the *application* (not just order the window out).
3. What the plain winit window lacks is behaviour *around* focus, which can be set natively on the existing `NSWindow`
   without replacing it (done in `gui/mac.rs::style_launcher_window`): status window level (above other floating windows),
   `CanJoinAllSpaces | FullScreenAuxiliary | IgnoresCycle` so it appears on the current Space and over full-screen apps,
   no hide-on-deactivate, clear background, shadow.

## Recommendation

Keep the egui/winit window. An NSPanel with `NSWindowStyleMaskNonactivatingPanel` would avoid activating Ashdash at all, but
it needs a custom hosting view for wgpu/egui input (IME, key events), which is a large and risky rewrite for a problem
we could not reproduce. Revisit if any of these show up in the field:

* the launcher appears behind a full-screen app on some Space/monitor (not exercised here: we only tested ordinary Spaces);
* the previous app's *text selection / field focus* is lost on hide (we only checked which app is frontmost);
* hotkey-to-first-key latency above ~100 ms on release builds with a cold GPU.

Not verified: full-screen apps, multiple physical monitors, release build timings, secure-input fields.

## Other native-window observations (smoke runs, debug build)

* Menu-bar status item (plain objc2 `NSStatusItem`, no `tray-icon`): menu showed "Show Ashdash, Reload, Preferences… (disabled), Quit Ashdash"; clicking Reload and Show Ashdash through System Events worked and fed the egui loop. No Dock icon.
* Large Type: Cmd+L stretched the window over the whole monitor under the mouse (covering the menu bar), dimmed it, centred the text at the maximum size; Esc returned to the normal launcher.
* Quick Look: Cmd+Y and a lone Shift tap (held ~150 ms; a sub-frame tap can be missed) spawn `qlmanage -p`. The launcher stays open while the preview has focus; Esc in the preview ends the process and the launcher takes focus back. The launcher window is drawn above the preview window (status window level).

## Launcher stays on screen after another app is activated (fixed)

Symptom (QA 3.4.1, `tests/e2e/qa/checklist.sh` s34): after `set frontmost of process "Finder" to true` the launcher stayed
up in 3 of 5 trials; Cmd+Tab hid it every time.

**Root cause.** The hide decision lived in `LauncherApp::ui` and read: `focused == Some(false) && shown_at.elapsed() > 400 ms`.
The 400 ms were a grace period for the stale `false` a hidden window reports until AppKit makes it key (6-50 ms). But a
real loss inside that period was thrown away and *never looked at again*: nothing asks for another frame, and egui draws
a visible, unfocused, untouched window only when something repaints it. The focus change itself repaints once (egui-winit
`WindowEvent::Focused` -> `repaint: true`, and on macOS `viewport.focused` is `NSWindow.isKeyWindow`, read fresh in every
frame), and that one frame is exactly the one the rule ignored. Whether a trial failed was therefore a matter of how
long after `show()` the other app took the focus: an `osascript` activation started right after the hotkey needs about
340 ms (inside the grace), a Cmd+Tab typed by a script about 420-490 ms (outside), which is the "3 of 5 / 5 of 5" split.
A user who clicks another app within 400 ms of showing the launcher hit it as well.

Evidence (lab VM, release bundle, `ASHDASH_TRACE=1`, `tests/e2e/focus/probe.sh`; the log of one failing trial, with the
per-frame lines added by the fix to show the missing frame):

```
[trace  4155ms] show
[trace  4164ms] focus=Some(true)  +8ms after show        <- window is key
[trace  4501ms] focus=Some(false) +345ms after show      <- Finder activated: frame drawn, inside the 400 ms grace, ignored
                                                            ... no further frame, no hide; the window stays up until something else repaints
```

| Before the fix (old rule) | result |
|---|---|
| activate Finder as in 3.4.1 (about 340 ms after show) | 1 of 20 hid (first run: 0 of 5) |
| same, but activation delayed by 0.3 s (about 660 ms after show) | 6 of 6 hid, `hide` logged in the same frame as `focus=Some(false)` |
| Cmd+Tab (arrives 415-490 ms after show) | 20 of 20 hid |

Not the cause: the NSApplication/NSWindow notifications (winit turns them into the `Focused` event we saw arrive on time
in every trial), Quick Look, `hide_app`, throttled repaints of invisible windows (the window is visible here).

**Fix** (`gui/focus.rs`, `FocusWatch`, called from `LauncherApp::ui`; no polling, no new native observer):

* `false` after the window was seen `true` since `show()` is a loss at once, whenever it happens.
* `false` before it has ever been `true` still gets the 400 ms grace, and the frame returns `Recheck(rest)`, so the app
  calls `request_repaint_after(rest)`: one frame at the end of the grace period decides. Never a repeating timer.
* While a Quick Look preview is open the watch does not hold the focus; when the preview ends, `tick_quicklook` restarts
  `shown_at` and the launcher gets the grace period again to take the focus back (a first version that kept `gained` across
  the preview hid the launcher in the frame after the preview closed; the lab check `quicklook-N` caught it).
* The decision is a pure function, unit-tested (`gui::focus::tests`: loss 345 ms after show, loss at 1..5000 ms, stale
  `false` right after show, never focused, unknown focus, Quick Look in and out, a second `show`).

Verification in the lab (macOS 15.7.7 guest, bundle built with the fix, one VM at a time):

| Check | Before | After |
|---|---|---|
| 3.4.1 Finder activated, `probe.sh --trials 20` | 1/20 | 20/20 (and 5/5 in the unmodified `checklist.sh s34`) |
| 3.4.2 Cmd+Tab, `probe.sh --trials 20` | 20/20 | 20/20 (5/5 in `checklist.sh s34`) |
| activation delayed 0.06 s / 0.3 s after show | - / 6 of 6 | 10/10 / 10/10 (+10/10 Cmd+Tab at 0.3 s) |
| Finder activated 0, 90, 150, 300 ms after the hotkey was sent (`--race`) | - | 8/8 each: never stuck without focus, hotkey shows it again afterwards |
| Quick Look (Cmd+Y on a file) takes the focus; Esc in the preview | passes | 3/3: launcher stays (lowered below the preview), is raised and focused again |
| actions panel (Right), Esc closes the panel, Esc/Esc hides | passes | 3/3 |
| click inside the launcher keeps it and its focus | passes | passes |
| idle CPU, hidden, 60 s | - | 0.01 CPU-s (ps cputime) |
| idle CPU, shown and focused, 30 s | 0.34 CPU-s | 0.33-0.40 CPU-s (caret blink; same as before, the extra trace line only costs when `ASHDASH_TRACE` is set) |
| `checklist.sh s33 w45` (keys, Large Type, held Return, import prompt, URL consent) | - | no new failures; 3.3.3b (Tab on an app row) is fixed separately |

Re-run: `tests/e2e/lab-run <vm> -- bash tests/e2e/focus/probe.sh --trials 20` (`--after-show S`, `--race MS`, `--only act|tab`),
`bash tests/e2e/focus/regress.sh [--only ql|panel|click|idle]`; both build `Ashdash.app` into `~/focus-dist` (`FOCUS_DIST`).
`ASHDASH_TRACE=1` now also prints one `frame focused=... +Nms after show` line per drawn frame while the launcher is shown.

Lab notes from this work: (1) in one clone `tcc-grant.sh` was not enough for System Events keystrokes (`1002 not allowed to send
keystrokes`): `sudo killall tccd` leaves the *system* `tccd` (pid started at boot) running with its old view; `kill -9` on it
(launchd restarts it) fixed it, the probe scripts do that after the grant. (2) `ashdash://` URLs (`open ashdash://...`) go to
whichever `Ashdash.app` LaunchServices last registered: with two bundles in the guest the URL consent checks (W5.4/W5.5) talk to
the wrong one; delete the other bundle or `lsregister -f` the one under test. (3) `lab-up` VMs are reaped after 30 minutes unless
started with `--max-age`.

## Preferences window

Decision: the preferences window is **its own process** (`ashdash prefs-window`, started by `ashdash prefs`, the status-item
menu item "Preferences…" and `ashdash://prefs`), using winit's *Regular* activation policy. A second eframe viewport inside the
launcher process was rejected: with the launcher's root window hidden eframe runs no UI pass (`show_ui` is false unless a child
viewport is already visible), so a child could not even be created on demand, and `NSApp.hide` (our focus-return trick) would
hide it together with the launcher. As a separate process it gets a Dock entry, a menu bar and Cmd+Tab while open, and cannot
change the launcher's level, auto-hide or focus behaviour. Verified on the desktop (debug build, temp HOME): the launcher opened over the open
preferences window with the hotkey/`ashdash show`, kept its own behaviour, and the preferences window stayed where it was.

The processes talk only through files in the data folder: the window edits `config.json` (valid edits only), the launcher's
settings watcher (300 ms debounce) applies it and answers in `run/reload-status.json`. A second `ashdash prefs` brings the open
window forward (flock on `run/prefs.lock`). Note for desktop scripting: the egui window has no accessibility tree, and
screenshots of this machine are 1.57 px per point; read the window frame through System Events before computing click positions.

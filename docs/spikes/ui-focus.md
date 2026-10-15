# Spike: does the egui window take key focus and give it back?

Question (WP7 item 1): when the global hotkey shows the launcher, does the eframe/winit window reliably become the
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

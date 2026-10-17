# Ashdash QA guide

Two parts: how to run the automated suites, and a manual checklist for what automation cannot
see (hotkey, focus, permissions). Security findings referenced as `SEC-nn` live in
[SECURITY.md](SECURITY.md).

## 1. Automated suites

| Command | What it runs | Time |
|---|---|---|
| `scripts/check.sh` | fmt, clippy `-D warnings`, `cargo test --workspace`: the gate for every change | ~1 min cold |
| `cargo test -p ashdash-core` | unit tests + all integration suites below | seconds |
| `cargo test -p ashdash-core --test engine_e2e` | public `Engine` API in a temp environment: ranking, exclusivity precedence, usage learning across engine instances, destructive-command confirmation (never run), triggers | <1 s |
| `cargo test -p ashdash-core --test import_security` | `import_workflow` / `import_package` with hostile synthetic zips: `../`, absolute paths, symlinks and chains, bundle-id tricks, malformed archives | <1 s |
| `cargo test -p ashdash-core --test scripts_security` | script argument passing, `{query}` escaping, env, graph limits, process lifecycle, `open` helper guards | <1 s |
| `cargo test -p ashdash-core --test import_staging` | the staged import: the installed bytes equal the previewed bytes even if the package file is swapped meanwhile; replacement and saved-settings notices; Write File and clipboard in the trust summary | <1 s |
| `cargo test -p ashdash-core --test clipboard_privacy` | concealed/transient/ignored-app copies never reach rows, usage or disk (SEC-19) | <1 s |
| `cargo test -p ashdash-core --test prefs_core` | the core half of the preferences window: live `apply_config` (providers, web searches and their URL check, restart-required keys, usage switch, clipboard settings), the file watcher, workflow list/enable/remove, package preview, themes, snippet library | ~2 s |
| `cargo test -p ashdash-core --test ui_glue_api` | engine behaviour behind the launcher glue: "open launcher" outcomes, a Script Filter's running text, Open With listing, Quick Look through the `Os` handle (fake OS) | <1 s |
| `cargo test -p ashdash-core --test fuzz` | proptest: `calc::eval`, `fuzzy::score`, `percent_encode` round trip, config load/migration on random JSON and bytes, Script Filter fed arbitrary bytes / wrong JSON types / mangled JSON | ~3 s |
| `cargo test -p ashdash-core --test compat_packages -- --nocapture` | imports every `*.alfredworkflow` bundled in an installed `/Applications/Reference.app` into a temp dir, loads it, generates keyword items, runs each graph, and prints the node types seen vs unsupported. Skips (passes) when the reference launcher is not installed | ~1 s |

Environment variables: `PROPTEST_CASES=N` scales the fuzz suites (try 20000 before a release);
`RUST_BACKTRACE=1` as usual.

Rules the suites follow (keep them when adding tests): everything lives in a unique temp dir
(`tests/common/mod.rs`: `TempDir`, `Env`, `Paths::under`); never touch the real `HOME`, the user's
clipboard, or `~/Library/Application Support/reference-launcher`; never execute destructive actions (shutdown, logout,
trash); no reference launcher assets are copied into the repo; the compat suite only reads the installed app.

### Known-issue tests

Tests with `#[ignore = "known: SEC-nn ..."]` demonstrate an open finding and are **expected to fail**
today. Run them to check whether a fix landed:

```
cargo test -p ashdash-core --tests -- --ignored --no-fail-fast
```

Caveats: `zip_bomb_is_rejected` writes ~300 MB to a temp dir, `huge_script_output_is_bounded` allocates
~200 MB, `timeout_kills_grandchildren` / `unread_stdin_cannot_defeat_the_timeout` / the pipe test take 6-14 s, and
`notification_text_cannot_inject_applescript` may flash a notification (it only `touch`es a temp file).
`calc_deep_nesting_*` and `fuzzy_pathologically_*` re-run the test binary as a child process, because a stack
overflow aborts the process. When a finding is fixed, delete the `#[ignore]` in the same commit.

`documents_*` tests pin intentional reference-compatible behaviour (for example that the default query mask
leaves `;` and `|` live). If one fails because behaviour got *safer*, update the test and SECURITY.md together.

## 2. GUI smoke test (scripted, one window, one machine)

```
QA_GUI_OK=1 scripts/qa/gui-smoke.sh            # types "saf"
QA_GUI_OK=1 scripts/qa/gui-smoke.sh "vsc"      # other query
```

It builds the debug binary, starts it with a throwaway `HOME`, presses Option+Shift+Space via System Events,
types the query, takes a screenshot, prints the screenshot path on the last line, then kills the app and
deletes the throwaway HOME. Requirements and cautions:

- The terminal needs **Accessibility** (to send keys) and **Screen Recording** (for `screencapture`) permission.
- It owns the global hotkey and the keyboard for ~8 s: close other Ashdash instances; do not run two copies of it,
  and do not run it while another agent or person is using the screen.
- The screenshot is the **entire desktop**. Open it, inspect it, delete it. Never commit it or attach it to a PR.
- It passes if the app is still alive after hotkey and typing; *what is on screen* is judged by a human.
  Look for: the launcher visible near the top centre, query text "saf", Safari first in the list, no error row.
- The clipboard watcher runs against the real pasteboard; its history goes into the throwaway HOME and is deleted.

`scripts/qa/ui-glue-smoke.sh [binary]` is the longer wave 3 version (actions panel and buffer, in-window import prompt with
a synthetic workflow, Script Filter rerun and pending rows, Quick Look layering, held-modifier subtitle, scrolling Large Type).
It shows windows and drives System Events on the **host desktop**, so it takes the lock `/tmp/ashdash-desktop.lock`
(`mkdir`; always `rmdir` it afterwards), runs with a throw-away HOME and a non-default hotkey, and leaves whole-desktop
screenshots in a temp folder that you look at and delete. Run it from a clean desktop session, not from CI. Once the VM lab
(`docs/LAB.md` on branch `wp/wp14-lab`, not merged) is available, GUI checks belong in the lab instead.

## 3. Manual checklist

Run on a debug build (`cargo run -p ashdash`) with a clean profile
(`HOME=$(mktemp -d) cargo run ...` after building, or delete `~/Library/Application Support/ashdash`).
Mark each item OK / FAIL / n.a. and note the macOS version and display setup.

### 3.1 Hotkey and window
- [ ] Option+Shift+Space shows the launcher within ~100 ms, input field focused, cursor blinking.
- [ ] Pressing the hotkey again hides it. Repeat 20 times quickly: no stuck state, no flicker, no second window.
- [ ] Shows on the screen containing the mouse (multi-monitor) and on the current Space; also over a full-screen app.
- [ ] After hide, focus returns to the previously active app (type a character: it lands there).
- [ ] Hotkey registration failure (start a second instance, or bind the same combo elsewhere): the user sees a clear message, not silence.
- [ ] Changing `hotkey` in `config.json` or in Preferences > General (e.g. `cmd+shift+space`) takes effect within about a second without a restart; the old key stops working, the new one works, the start-up hint (if shown) names the new key. An invalid value keeps the old key and reports (notification, Preferences banner).
- [ ] No Dock icon, no Cmd+Tab entry (unless intended).

### 3.2 Typing and results
- [ ] `saf` → Safari first. `vsc` → Visual Studio Code if installed (initials). `visual studio` → same.
- [ ] `2+3*4` → 14, Return copies the result. `= sqrt(81)` → 9. `1/0` and `((((` show nothing and do not crash.
- [ ] Pasting a very long text (100 KB), or 5,000 `(`, does not hang or crash the app (SEC-08/09).
- [ ] Unicode: `ü`, emoji, CJK, an IME composition (Japanese/Chinese) do not lose characters or crash.
- [ ] Backspace to empty clears results; Cmd+A, Cmd+Backspace edit as in any text field.
- [ ] Typing fast (hold a key) keeps up: no results from an older query overwriting the newest.
- [ ] `g rust`, `wiki x` offer web searches; fallback searches appear last for unknown text.
- [ ] `open <name>` / `find <name>` return Spotlight files; `~/Doc` browses paths; `> echo hi` offers Terminal.
- [ ] System commands: `lock`, `sleep` work; `shutdown`, `restart`, `logout`, `empty trash` ask for confirmation
  and do nothing on Esc. **Test the destructive ones only with confirmation declined.**

### 3.3 Keyboard
- [ ] Up/Down and Ctrl+P/Ctrl+N move the selection; selection scrolls into view; wraps or stops consistently.
- [ ] Return runs the selected item and hides the launcher; items that need a second Return show the prompt.
- [ ] Tab autocompletes the selected item's title/autocomplete text into the field.
- [ ] Esc hides the launcher (first Esc clears the text if that is the chosen design: note it).
- [ ] Cmd+1…Cmd+9 run the visible result in that row; the number hints match the rows.
- [ ] Cmd+L shows Large Type for the selection; Esc/click dismisses it. With a very long text (several screens) the text scrolls with the wheel, arrows and Page keys.
- [ ] Hold Return on Shut Down / Restart / Empty Trash (do not release): it asks once and **never** confirms by repeating; releasing and pressing Return again confirms (decline with Esc; do not run it for real).
- [ ] Modifier overrides (Cmd/Alt/Ctrl/Shift+Return) on a workflow Script Filter item use the `mods` entry.
- [ ] Cmd+Q quits; Cmd+W/Cmd+H do not leave the app in an odd state.

### 3.4 Focus loss
- [ ] Clicking another window hides the launcher; no ghost window remains in Mission Control.
- [ ] Switching Spaces / Cmd+Tab while open hides it and does not steal focus back.
- [ ] Screen lock/unlock, display sleep and wake, and display hot-plug while the app runs: hotkey still works.

### 3.5 Clipboard history and snippets
- [ ] `clip` lists recent copies (use harmless text). Newest first; duplicate copies move to top.
- [ ] Return on an entry copies it; Cmd+V into a text editor pastes it.
- [ ] Copy from a password manager (1Password/Bitwarden "copy password"; marked concealed): the entry must **not**
  appear in `clip` nor in `clipboard.json` (SEC-19; the code and its tests exist, **this check with a real password manager has never been done**).
- [ ] Per-app ignore list: copies made in an app listed in `clipboard.ignored_apps` (or added in Preferences > Clipboard) are skipped, without a restart.
- [ ] `ls -l@ ~/Library/Application\ Support/ashdash/*.json` shows `-rw-------` for `clipboard.json` and `usage.json`.
- [ ] Image / file copies: a title is shown, copying back restores the original type.
- [ ] `snip` lists snippets from `snippets.json`; `{clipboard}`, `{date}`, `{time}`, `{datetime}` expand; `{cursor}` handling.
- [ ] Auto-paste: Return on a `clip`/`snip` row pastes into the frontmost app with Accessibility granted; with it denied the error says so. (Real Cmd+V behaviour is still unverified.)

### 3.6 Workflows
- [ ] `ashdash import x.alfredworkflow` for a trusted sample workflow prints the trust summary, asks `Install this workflow? [y/N]`, and installs only on `y`; with stdin not a terminal and without `--yes` it refuses; `--yes` installs without asking. The keyword works afterwards (within a second, no reload needed).
- [ ] A package with `../` or absolute entry names (build with `tests/common` helpers or `zip`) is rejected or neutralised;
  nothing appears outside `workflows/`.
- [ ] A Script Filter that prints garbage shows an error row, not a crash; one that hangs gives up after 10 s.
- [ ] Disabling a workflow removes its keywords; `ashdash trigger <bundle> <trigger> arg` runs the external trigger.
- [ ] The import prompt (every path: `ashdash import`, double-click / Open With in Finder, drop on the launcher window, Right Arrow > "Install Workflow..." on a file result, Preferences > Workflows) lists scripts, Terminal commands, Write File targets, clipboard replacement, URL schemes, external files and external triggers. A package whose only object is Write File must **not** say "Runs no scripts and no terminal commands".
- [ ] Launcher prompt: Return does nothing for the first 600 ms; a Return held from selecting "Install Workflow..." never installs; a fresh press installs; Esc cancels and leaves nothing in `<data>/import-staging/`.
- [ ] Re-importing the same bundle id says "Replace workflow" and "This replaces ..."; with a `workflow-config/<bundle id>.json` present it also warns that saved settings are kept.
- [ ] Replace the package file on disk while the prompt is up, then confirm: the workflow that was described is the one installed.
- [ ] A name with a right-to-left override or 10,000 characters is shown cleaned and cut in all prompts.
- [ ] Editing `workflow_dirs` in `config.json` registers the new folder's keywords and hotkeys within about a second (no prompt, by design) and removing the entry removes them.

### 3.7 Permissions (macOS prompts)
Test with a fresh app identity if possible (`tccutil reset All <bundle id>` for a signed build).
- [ ] Notifications (`output.notification` uses `osascript`): the first call may prompt for Script Editor/osascript notification permission; denial does not crash.
- [ ] Automation prompts when Terminal (`>`), Finder (empty trash), System Events (logout/restart) are first driven; denial shows an error row.
- [ ] Accessibility: required for auto-paste / snippet expansion; the app explains how to grant it and works without it otherwise.
- [ ] Contacts / Files & Folders / Full Disk Access prompts only appear for features that use them.
- [ ] Gatekeeper: a downloaded build opens after the documented steps (signing/notarisation is a later work package).

### 3.8 Appearance and robustness
- [ ] Light/dark mode switch while open; a custom `.alfredappearance` theme loads; a broken theme falls back.
- [ ] Retina and non-Retina displays; text scaling; the window never goes off-screen.
- [ ] Idle CPU near 0 % (Activity Monitor, 1 min idle), memory stable after 1,000 queries.
- [ ] Kill -9 during a clipboard write, then relaunch: history loads or resets, no crash.
- [ ] Corrupt `config.json` (`echo '{' > ...`): the app starts with defaults and says so on stderr.
- [ ] Start with no network: nothing blocks (web searches only open URLs).

### 3.9 Universal Actions panel and buffer
- [ ] Right Arrow (cursor at the end) on a file, folder, URL, calculator result and a clipboard text row opens the panel with the title "Actions for ..."; typing filters it; Left (cursor at start) or Esc goes back to the previous list and selection.
- [ ] Right on "Open With..." lists applications; Return opens the file with the chosen one.
- [ ] Option+Up adds the selected file to the buffer (footer shows the count), Option+Down removes it; with a non-empty buffer Right opens the buffer actions; hiding and showing the launcher empties the buffer.
- [ ] Move to Trash from the panel names the full path, needs a second *fresh* Return (test with a throw-away file in a temp folder), and works; the file is in the Trash.
- [ ] Actions are not offered for system commands, workflow results and `> cmd` rows; nothing run from the panel changes the ranking of the original result.

### 3.10 Preferences window and live reload
- [ ] Menu-bar item > Preferences..., `ashdash prefs` and `open ashdash://prefs` open one window with its own title bar (drag it; the traffic lights stay) and a sidebar (General, Search, Clipboard, Snippets, Workflows, Permissions, About; Cmd+1...7 switch, Cmd+K focuses "Go to a setting..."; dark and light follow the system); a second request brings it forward instead of opening another. The launcher hotkey still behaves as before while the window is open.
- [ ] General (default 760x560 window, attention card shown: no scrolling; at 640x480 nothing is cut off, it may scroll): the shortcut is one box. Click it, or Tab to it and press Return or Space: orange outline, "Press the new shortcut...", "was ..." at the right; Esc cancels; Backspace sets the default; a bare letter shows the refusal and the red row outline; a system shortcut such as Cmd+Space is warned about; pressing Cmd+2 while recording records it and does not switch pages. Launch at login (bundle only), theme: dropdown and "Import theme..." only (no path field; dropping a file on the window still imports), results per search, switch learning off (and check that `usage.json` is still there), press "Forget usage..." (the file is emptied).
- [ ] Permissions (Cmd+6): Accessibility, Automation (System Events, Finder, Terminal) and Full Disk Access each show a light and one of Granted / Not granted / Asked on first use / Unknown. **No macOS prompt may appear while the page is open or on Cmd+R** (watch for a dialog; this is the point of the check). Revoke Full Disk Access for Ashdash in System Settings, restart, with Safari bookmarks on: General shows the "Full Disk Access not granted" card, the DISK ACCESS light, and Return (nothing focused) or "Fix..." opens this page; "Open System Settings" opens the matching pane; after granting and restarting Ashdash the card and light are gone. Needs a real Mac with a signed or ad-hoc bundle; the VM lab only exercises the probes (`docs/design/settings-v4/tools/lab-shots.sh`).
- [ ] Features: switch `calc` off and on; add a web search with a bad URL (`file:///x/{query}`) and see it refused; add a good one and use its keyword at once; change clipboard retention and an ignored app; add/edit/delete a snippet.
- [ ] Workflows: enable/disable a workflow (its keyword appears/disappears within a second), "Configure..." saves a value, "What can it do?" shows the summary, Remove asks twice.
- [ ] Break `config.json` by hand (`echo '{' > ...`): the launcher keeps working with the previous settings, one notification appears, the preferences window shows the problem, and fixing the file clears it. Change `app_dirs`: the window says a restart is needed.

### 3.11 Release-candidate measurements that need the real process (see docs/PERF.md)
- [ ] Idle memory of the running launcher: start `ashdash` (bundle or release binary), wait 10 s, then `ps -o rss=,cputime= -p "$(pgrep -x ashdash)"` twice, 60 s apart. Target from the plan: RSS under 80 MB, CPU time not growing. Record the app count of the machine (`ashdash query` does not print it; count `/Applications`).
- [ ] Hotkey-ready time: from process start to the first Option+Shift+Space that shows the window (stopwatch or `log stream`); target under 300 ms with a warm cache.
- [ ] `mdfind` file search on a real home folder: time `open <name>` and `in <text>`; note what happens when a search hits the 4 s Spotlight limit (a huge home folder).
- [ ] Download the DMG from a draft release in a browser (so it is quarantined), install it as in docs/release-notes/v0.1.0.md, and confirm the documented Gatekeeper steps work on a Mac that never saw the project.

## 4. Reporting

File defects with: build (`git rev-parse --short HEAD`), macOS version, steps, expected/actual, and logs
(`ashdash > app.log 2>&1`). Security-relevant ones go to SECURITY.md with a severity and, where possible,
an `#[ignore = "known: SEC-nn ..."]` regression test.

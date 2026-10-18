# Architecture

A map of the code for people who want to change it. For how to use the product see [USER_GUIDE.md](USER_GUIDE.md); for the
product values see [PRODUCT.md](PRODUCT.md); for the threat model [SECURITY.md](SECURITY.md).

## 1. Two crates

```
Cargo.toml                    workspace: crates/core, crates/app
crates/core   (ashdash-core)  the engine: no UI, no windowing; fully testable without a display
crates/app    (ashdash)       the binary: egui launcher window, preferences window (its own process), global hotkeys, CLI, control socket, URL scheme, AppleScript
packaging/                    app bundle, DMG, notarization, Homebrew cask templates, icon generator
scripts/                      check.sh (the gate), gen-notice.py, qa/
docs/                         this documentation
```

### `crates/core/src`

| Module | Role |
|---|---|
| `engine.rs` | `Engine`: owns config, providers, shared stores; `query` (reads one `Arc<Live>` snapshot of config + providers), `execute`, `reload` / `reload_content_background`, `apply_config` (live settings), `config_now`, `stage_workflow` / `install_staged` (confirmed import; `import_workflow` installs without asking and is for trusted callers), `run_trigger` (External Triggers), `run_node` (workflow hotkeys, by workflow id), workflow settings, `actions_for`, `clear_usage` |
| `providers/` | one file per feature; `mod.rs` holds the `Provider` trait and `registry_for` (the single place providers are registered) |
| `item.rs` | `Item` (a result row), `Action` (what Return does), `Icon`, `Mods` (modifier flags) |
| `config.rs` | `Paths`, `Config` (schema v2, migration, atomic 0600 writes, `edit_locked` read-modify-write under `run/config.lock`), `FileSearchSettings` (incl. the clamped run budget), `ClipboardSettings`, `learn_usage` |
| `settings_view.rs` | dotted-path get/set/keys on the config as JSON (behind `ashdash config`); everything the preferences window needs from core: `SettingsWatcher` (300 ms debounce, classifies config vs content changes), `changed_keys`, workflow list / enable / remove / settings sheet, theme and snippet-library helpers |
| `confine.rs` | `confined_file`: a relative path must stay inside a root (no `..`, no symlinks below it); used for app and workflow icons (SEC-17) |
| `apps.rs`, `index/` | application records; persistent, FSEvents-watched app index, icon cache |
| `fuzzy.rs` | scoring: prefix, word prefix, initials, substring |
| `usage.rs` | learned ranking (`usage.json`); `set_enabled` (config `learn_usage`) and `clear` |
| `calc.rs` | expression parser and evaluator with depth/length limits |
| `files.rs` | Spotlight (`mdfind`) search with scope, kind filters, budgets; path browsing |
| `bookmarks/` | Safari, Chromium family, Arc, Firefox readers behind one cache |
| `clipboard/` | history (text, images, file lists), concealed-type filter, snippets, placeholders, import, auto-paste |
| `actions.rs` | Universal Actions and the `Os` trait (every OS side effect goes through it so tests can fake it) |
| `system.rs` | system commands (lock, sleep, log out, ...) with confirmation rules |
| `websearch.rs` | keyword web searches, percent-encoding, and `validate_url_template` (the one rule for search URLs; invalid searches are skipped when the providers are built) |
| `theme.rs` | appearance-file (theme) parsing and the built-in theme; see [WORKFLOWS.md](WORKFLOWS.md#file-formats-and-identifiers) |
| `workflow/` | the workflow engine ([WORKFLOWS.md](WORKFLOWS.md)): `model` (load), `package` (safe import; `stage_package` opens a package once into a private staging folder, `StagedPackage` is previewed and installed from that same copy), `filter` (keyword / Script Filter / list / file filters), `exec` (graph walk and effects), `script` (running interpreters safely), `utility`, `xml`, `trust` (`TrustSummary` for the import prompt), `userconfig`, `compat` |
| `brand.rs` | product name, slug, URL scheme, bundle id, default hotkey |

### `crates/app/src`

| Module | Role |
|---|---|
| `main.rs` | CLI dispatch (`ashdash --help`), start-up of the launcher |
| `gui/` | the launcher window (eframe/egui): `mod.rs` app state and loop, `window.rs` show/hide/hotkey, `keys.rs` (incl. fresh-press detection), `results.rs`, `style.rs`, `mac.rs` (native `NSWindow` styling, vibrancy), `statusitem.rs` (menu bar), `largetype.rs`, `quicklook.rs`, `geometry.rs` (monitor placement), `keynames.rs` (hotkey spec to glyphs), `rowhint.rs`, `workers.rs` (query worker), `schedule.rs` (Script Filter rerun scheduling), `panel.rs` (Universal Actions panel and buffer), `importprompt.rs` (import prompt wording and rules), `glue.rs` (state for panel, buffer, prompt), `trace.rs` |
| `gui/prefs/` | the **preferences window**, a separate process (`ashdash prefs-window`): `app.rs` (window, banners, drops), `general.rs`, `features.rs`, `workflows.rs` (pages), `hotkey.rs` (recorder, re-registration), `single.rs` (one window), `status.rs` (launcher status file); and, running inside the **launcher**, `reload.rs` (live reload) |
| `prefs_model.rs` | pure view-model of the preferences window (validation, edits through `Config::edit_locked`), unit-tested without a window |
| `ipc.rs` | single-instance lock and the Unix socket protocol used by the CLI (verbs `ping`, `show`, `search`, `trigger`, `reload`, `quit`, `theme`, `prefs`, `clear-usage`) |
| `urlscheme.rs` | strict `ashdash://` parser, rate limit, per-workflow consent |
| `scripting.rs` | AppleScript / JXA command handlers (Cocoa scripting) |
| `hotkeys.rs` | workflow global hotkeys on the shared hotkey manager |
| `events.rs`, `automation.rs` | the one channel through which all outside callers reach the window; production wiring |
| `clipboard_watch.rs` | `NSPasteboard.changeCount` poller feeding the core history |
| `mac/` | login item (`SMAppService`), double-clicked documents (workflow packages, appearance files, snippet packages) with the native import dialog |

Rule of thumb: anything that needs a display or AppKit lives in `app`; anything that decides what to show or do lives in `core`
behind traits so it can be tested in a temp directory.

## 2. The provider model

A provider is a small object that answers "given this query text, what rows do you offer?".

```rust
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;                         // stable id, also the name used in `providers_disabled`
    fn learns(&self) -> bool { true }                     // do picks from this provider feed usage ranking?
    fn query(&self, ctx: &QueryCtx) -> ProviderOutput;    // items + `exclusive` flag
    fn placement(&self) -> Placement { Placement::Ranked }// Ranked, or Trailing (appended after truncation)
}
```

* **Exclusive** output (`> cmd`, `open x`, `clip`, `bm`, a matching web keyword, `actions`) is the whole answer: later
  providers are not asked. The engine only truncates it to `max_results`.
* Otherwise rows from all providers are pooled. Providers with `learns() == true` get a usage boost; the rest keep their own
  score. The pool is sorted, truncated to `max_results`, and `Trailing` rows (fallback web searches) are appended.
* Order of the registry (`PROVIDER_IDS`): `workflow`, `shell`, `files`, `actions`, `clipboard`, `snippets`, `bookmarks`,
  `web-keyword`, `path`, `calc`, `apps`, `system`, `web-fallback`.
* Providers see read-only shared state through `EngineState` (config, paths, app list, workflows, snippets, clipboard history).
  OS side effects (`mdfind`, `open`, Finder scripts) go through `RegistryDeps::os`, so tests inject a fake and a temporary home
  and never touch the real machine.

## 3. What happens when you type and press Return

```
 keystroke ──► egui text field (UI thread)
                  │ request (generation n, text)                      newest request wins; stale ones dropped
                  ▼
            ┌───────────────┐   Engine::query(text)
            │ query worker  │───────────────────────────────┐
            │ (1 thread)    │                               ▼
            └───────▲───────┘        for p in providers (registry order)
                    │ (n, rows)         ├─ workflow  keyword / Script Filter (runs scripts, 10 s limit)
                    │                   ├─ shell     "> cmd"                        ── exclusive?  ─┐
                    │                   ├─ files     open/find/in/'  → mdfind        ── exclusive?  ─┤ return at once
 UI thread draws rows when              ├─ actions, clipboard, snippets, bookmarks, web-keyword, path
 generation matches                     ├─ calc, apps, system                  (pooled, usage boost,
                                        └─ web-fallback                         sort, truncate, +trailing)
                                                │
 Return ──► UI thread: Engine::confirmation_for(item, mods)
              │ needs confirmation? ──► show "press ↩ again" and wait (only a fresh press answers, never a key repeat)
              ▼
        action thread: Engine::execute(item, query, mods, confirmed)
              │ Action::Open/Reveal/OpenUrl/Copy/System/Shell        (OS helpers; `open`, `osascript`, pmset)
              │ Action::Clipboard(...)   history / paste via CGEvent  (Accessibility)
              │ Action::Universal(...)   through the `Os` trait
              │ Action::Workflow(...)    Workflow::run → walk the graph → Effects → apply_effects
              ▼
        Outcome { Done | LargeType(text) | ShowLauncher(query) | Confirm(msg) | Error(msg) } ──► UI (overlay, status line)
              └─ on success: Usage::record(query, item id)  (not for Universal Actions or clipboard rows, and not when `learn_usage` is off)
```

Right Arrow instead calls `Engine::actions_for(item)` and replaces the list with the actions (`gui/panel.rs` keeps a stack of levels
so Esc/Left restore the previous list); the actions are ordinary `Item`s with `Action::Universal`, so Return goes through the same
confirmation and `execute` path. A Script Filter's `rerun` makes the query worker (`gui/schedule.rs`, pure and tested) repeat the
newest request until the query changes or five minutes have passed.

Workflow graph walking (`workflow/exec.rs`): an explicit work stack (no recursion), at most 256 object executions per run,
outgoing wires ordered by canvas position, `Delay` yields. Scripts run in-line because their output feeds the next object; every other
side effect becomes an `Effect` value (`OpenUrl`, `Copy`, `Notify`, `LargeType`, `WriteFile`, ...) that `apply_effects` performs, which keeps
the graph logic testable without touching the OS.

## 4. Threads

| Thread | Started by | Does |
|---|---|---|
| UI / main | `main` → eframe | window, keyboard, status item, Carbon hotkey callbacks, Apple Events (URL scheme, AppleScript) |
| query worker | `gui::workers` | runs `Engine::query` for the newest request |
| action thread (one per Return) | `gui/mod.rs` | `Engine::execute`, so a slow action never blocks drawing |
| icon threads (one per missing icon) | `gui/results.rs` | `apps::app_icon_png` (bounded concurrency in the icon cache) |
| clipboard watcher | `clipboard_watch.rs` | polls `changeCount` every 300 ms, applies the concealed/ignore filter, records |
| app index | `core/index` | background rescan after the cached list is published; FSEvents watcher (`notify`) applies incremental changes |
| bookmark cache | `core/bookmarks/cache.rs` | background (re)load of browser files when stale |
| reload | `Engine::reload` | rescan apps, workflows, snippets; calls made meanwhile coalesce into one extra pass |
| control socket | `ipc.rs` | accept loop plus one short-lived thread per connection |
| workflow hotkey | `hotkeys.rs` | one worker per press (gathering the argument can take a while) |
| delayed workflow stream | `workflow::spawn_delayed` | continues after a Delay object |
| document import | `mac/opendocs.rs` | confirm dialog and import off the main thread |
| settings watcher | `settings_view::SettingsWatcher` (started by `gui/prefs/reload.rs`) | watches `config.json`, the workflow folders, workflow settings and snippets (`notify`, 300 ms debounce); content changes are re-read on its own thread (`Engine::reload_content`), settings changes are handed to the window thread |
| content reload | `Engine::reload_content_background` | the rescan after `workflow_dirs` changed, so `apply_config` returns at once |
| import staging / install | `gui/glue.rs`, `gui/prefs/workflows.rs` | opening a package and installing the confirmed copy happen off the UI thread |
| preferences window | separate process | its own UI thread and workers; shares no memory with the launcher |

Shared state in `Engine` is behind `RwLock` (apps, workflows, snippets: readers never wait for a scan) or `Mutex` (usage,
clipboard history). Writers do slow work before taking a lock and hold it only for the swap.

## 5. Everything that talks to the window

All outside entry points end in one `AppEvent` channel drained once per frame (`events.rs`):

```
 CLI (ashdash show|search|reload|quit|theme|trigger|prefs) ─► Unix socket ┐
 ashdash://… (Apple Event, validated, rate-limited)  ──────────────────┤
 AppleScript / JXA (sdef commands)  ───────────────────────────────────┼──► AppEvent ──► LauncherApp::poll_automation
 workflow global hotkeys (hotkeys.rs) ─────────────────────────────────┤
 menu-bar item (Show / Reload / Preferences / Quit)  ───────────────────┘
```

Details and trust levels: [AUTOMATION.md](AUTOMATION.md).

## 5b. The preferences process, live reload and import staging

**Two processes.** `ashdash prefs` (also the menu-bar item and `ashdash://prefs`, both via `AppEvent::Preferences`) starts
`ashdash prefs-window`, a normal "regular"-activation app with its own Dock entry (`gui/prefs/`). It was made a separate process because a
second egui viewport inside the launcher cannot be created while the launcher's root window is hidden, and the launcher's focus-return trick
would hide it ([spikes/ui-focus.md](spikes/ui-focus.md)). One window at a time (`flock` on `run/prefs.lock`).

**They talk through files.**

```
 prefs-window ──(Config::edit_locked, valid edits only)──► config.json ─┐
 ashdash config set / an editor ────────────────────────────────────────┤
 prefs-window ──► workflows/, snippets, themes (install, enable, remove)┤
                                                                        ▼
                                     launcher: SettingsWatcher (300 ms debounce)
                                        ├─ content change ─► Engine::reload_content (workflows, snippets)
                                        └─ settings change ─► window thread: Config::load_strict
                                              ├─ Engine::apply_config  (providers, web searches, file search, bookmarks,
                                              │                         clipboard retention, learn_usage, workflow_dirs)
                                              ├─ hotkey rebind (old key stays if the new one fails), theme reload
                                              └─ workflow hotkeys re-read (roots and reserved launcher key now in force)
 launcher ──► run/reload-status.json (problems, restart_required, active hotkey) ──► shown as banners in prefs-window
 prefs-window ──► socket {"cmd":"clear-usage"} ──► launcher wipes usage.json (the launcher owns the in-memory table)
```

A parse error or an invalid value keeps the previous settings (`Engine::apply_config` is not called) and is reported once. `Engine::query`
works on one `Arc<Live>` snapshot, so a query that is running while settings change finishes with the old providers.

**Import staging.** Every install path (`ashdash import`, the Finder dialog, the launcher prompt, the preferences window) calls
`Engine::stage_workflow` (`workflow::stage_package`): the package is extracted once, within the importer's limits, into a private
`import-staging/.import-*` directory (0700) and described (`PackagePreview`: manifest fields, `TrustSummary`, what it replaces, whether saved
settings exist). The UI (`gui/importprompt.rs::prompt_text`, shared and sanitised) asks about that `StagedPackage`; `Engine::install_staged`
moves exactly that tree into `workflows/` (swap with restore on failure). Declining drops the value, which deletes the staging copy.

**Config writers** (`Config::edit_locked`) take an `flock` on `run/config.lock`; a hand edit with an editor is not coordinated. Because
the file is watched, `config.json` is also an input channel: `workflow_dirs` activates workflows without an import prompt
([SECURITY.md](SECURITY.md) section 3).

## 6. Data directories

`Paths::system()` resolves them from the user's home (`dirs` crate) and `brand::SLUG`; tests use `Paths::under(tmp)`.

```
~/Library/Application Support/ashdash/        (mode 0700 where Ashdash creates it)
  config.json            settings, schema v2
  usage.json             query → chosen item counts
  clipboard.json         history;  clipboard-images/  PNGs (0700 dir, 0600 files)
  snippets.json          flat snippets;  snippets/  one file per collection
  workflows/             installed workflows: user.workflow.<bundle id>/
  import-staging/        private (0700) copy of a package while its import prompt is open; swept when older than a day
  workflow-config/       <bundle id>.json  user values of a workflow's configuration sheet
  workflow-data/         per-workflow data folder (the workflow data variable, see WORKFLOWS.md)
  themes/                themes installed by double-click or ashdash:// 
  url-trust.json         which workflows may be fired by ashdash://trigger
  run/ipc.sock, ipc.lock control socket and single-instance lock (or /tmp/ashdash-<uid>/ when the path is too long)
  run/prefs.lock         one preferences window at a time
  run/config.lock        serialises read-modify-write of config.json
  run/reload-status.json what the launcher tells the preferences window (problems, restart needed, active hotkey)
~/Library/Caches/ashdash/
  apps-index.json (versioned application index), icons/ (PNG cache, pruned to ~500 files), workflow-cache/ (the workflow cache variable), scripts/ (temporary script files, 0600)
  (the bookmark cache is in memory only)
```

## 7. Design decisions worth knowing

* **Coexist with other launchers**: own URL scheme, own data folder, own bundle id, different default hotkey.
* **egui kept for the window**, with native `NSWindow` styling applied afterwards (vibrancy, level, collection behaviour). An
  `NSPanel` rewrite was considered and rejected for now: [spikes/ui-focus.md](spikes/ui-focus.md). Preferences are a separate process for the same reason (section 5b).
* **Confirm what you install**: the thing previewed is the thing installed (staging), and the prompt is built from the object graph, not from what the package says about itself.
* **No network in the core.** Web searches and bookmarks open in the user's browser.
* **Compatibility first, safety where the format has none**: workflows keep the semantics existing packages rely on, but with import limits, process-group
  timeouts, output caps and protected environment names ([WORKFLOWS.md](WORKFLOWS.md), [SECURITY.md](SECURITY.md)).
* **One brand file** (`crates/core/src/brand.rs`) holds the product name, slug, URL scheme and bundle id. Changing the slug or bundle id moves the data folder
  and makes macOS ask for permissions again, so treat it as a migration. The packaging scripts read the same constants.
* **Format identifiers in one place**: the identifiers defined by the workflow package and appearance file formats live in `crates/core/src/format_ids.rs`; everything else uses the constants.

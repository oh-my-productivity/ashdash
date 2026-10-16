# Architecture

A map of the code for people who want to change it. For how to use the product see [USER_GUIDE.md](USER_GUIDE.md); for the
plan and history see [PLAN.md](PLAN.md); for the threat model [SECURITY.md](SECURITY.md).

## 1. Two crates

```
Cargo.toml                    workspace: crates/core, crates/app
crates/core   (ashdash-core)  the engine: no UI, no windowing; fully testable without a display
crates/app    (ashdash)       the binary: egui window, global hotkeys, CLI, control socket, URL scheme, AppleScript
packaging/                    app bundle, DMG, notarization, Homebrew cask templates, icon generator
scripts/                      check.sh (the gate), gen-notice.py, qa/
docs/                         this documentation, plan, reviews, briefs
```

### `crates/core/src`

| Module | Role |
|---|---|
| `engine.rs` | `Engine`: owns config, providers, shared stores; `query`, `execute`, `reload`, `import_workflow`, `run_trigger`, workflow settings, `actions_for` |
| `providers/` | one file per feature; `mod.rs` holds the `Provider` trait and `registry_for` (the single place providers are registered) |
| `item.rs` | `Item` (a result row), `Action` (what Return does), `Icon`, `Mods` (modifier flags) |
| `config.rs` | `Paths`, `Config` (schema v2, migration, atomic 0600 writes), `FileSearchSettings`, `ClipboardSettings` |
| `settings_view.rs` | dotted-path get/set/keys on the config as JSON (behind `ashdash config`) |
| `apps.rs`, `index/` | application records; persistent, FSEvents-watched app index, icon cache |
| `fuzzy.rs` | scoring: prefix, word prefix, initials, substring |
| `usage.rs` | learned ranking (`usage.json`), opt-out and clear at the API level |
| `calc.rs` | expression parser and evaluator with depth/length limits |
| `files.rs` | Spotlight (`mdfind`) search with scope, kind filters, budgets; path browsing |
| `bookmarks/` | Safari, Chromium family, Arc, Firefox readers behind one cache |
| `clipboard/` | history (text, images, file lists), concealed-type filter, snippets, placeholders, import, auto-paste |
| `actions.rs` | Universal Actions and the `Os` trait (every OS side effect goes through it so tests can fake it) |
| `system.rs` | system commands (lock, sleep, log out, ...) with confirmation rules |
| `websearch.rs` | keyword web searches and percent-encoding |
| `theme.rs` | `.alfredappearance` parsing and the built-in theme |
| `workflow/` | the reference-compatible workflow engine: `model` (load), `package` (safe import), `filter` (keyword / Script Filter / list / file filters), `exec` (graph walk and effects), `script` (running interpreters safely), `utility`, `xml`, `trust`, `userconfig`, `compat` |
| `brand.rs` | product name, slug, URL scheme, bundle id, default hotkey |

### `crates/app/src`

| Module | Role |
|---|---|
| `main.rs` | CLI dispatch (`ashdash --help`), start-up of the launcher |
| `gui/` | eframe/egui window: `mod.rs` app state and loop, `window.rs` show/hide/hotkey, `keys.rs`, `results.rs`, `style.rs`, `mac.rs` (native `NSWindow` styling, vibrancy), `statusitem.rs` (menu bar), `largetype.rs`, `quicklook.rs`, `geometry.rs` (monitor placement), `keynames.rs` (hotkey spec to glyphs), `rowhint.rs`, `workers.rs`, `trace.rs` |
| `ipc.rs` | single-instance lock and the Unix socket protocol used by the CLI |
| `urlscheme.rs` | strict `ashdash://` parser, rate limit, per-workflow consent |
| `scripting.rs` | AppleScript / JXA command handlers (Cocoa scripting) |
| `hotkeys.rs` | workflow global hotkeys on the shared hotkey manager |
| `events.rs`, `automation.rs` | the one channel through which all outside callers reach the window; production wiring |
| `clipboard_watch.rs` | `NSPasteboard.changeCount` poller feeding the core history |
| `mac/` | login item (`SMAppService`), double-clicked documents (`.alfredworkflow`, `.alfredappearance`, `.alfredsnippets`) |

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
              │ needs confirmation? ──► show "press ↩ again" and wait
              ▼
        action thread: Engine::execute(item, query, mods, confirmed)
              │ Action::Open/Reveal/OpenUrl/Copy/System/Shell        (OS helpers; `open`, `osascript`, pmset)
              │ Action::Clipboard(...)   history / paste via CGEvent  (Accessibility)
              │ Action::Universal(...)   through the `Os` trait
              │ Action::Workflow(...)    Workflow::run → walk the graph → Effects → apply_effects
              ▼
        Outcome { Done | LargeType(text) | Confirm(msg) | Error(msg) } ──► UI (overlay, status line)
              └─ on success: Usage::record(query, item id)  (not for Universal Actions)
```

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

Shared state in `Engine` is behind `RwLock` (apps, workflows, snippets: readers never wait for a scan) or `Mutex` (usage,
clipboard history). Writers do slow work before taking a lock and hold it only for the swap.

## 5. Everything that talks to the window

All outside entry points end in one `AppEvent` channel drained once per frame (`events.rs`):

```
 CLI (ashdash show|search|reload|quit|theme|trigger) ──► Unix socket ──┐
 ashdash://… (Apple Event, validated, rate-limited)  ──────────────────┤
 AppleScript / JXA (sdef commands)  ───────────────────────────────────┼──► AppEvent ──► LauncherApp::poll_automation
 workflow global hotkeys (hotkeys.rs) ─────────────────────────────────┤
 menu-bar item (Show / Reload / Quit)  ────────────────────────────────┘
```

Details and trust levels: [AUTOMATION.md](AUTOMATION.md).

## 6. Data directories

`Paths::system()` resolves them from the user's home (`dirs` crate) and `brand::SLUG`; tests use `Paths::under(tmp)`.

```
~/Library/Application Support/ashdash/        (mode 0700 where Ashdash creates it)
  config.json            settings, schema v2
  usage.json             query → chosen item counts
  clipboard.json         history;  clipboard-images/  PNGs (0700 dir, 0600 files)
  snippets.json          flat snippets;  snippets/  one file per collection
  workflows/             installed workflows: user.workflow.<bundle id>/
  workflow-config/       <bundle id>.json  user values of a workflow's configuration sheet
  workflow-data/         per-workflow data folder (alfred_workflow_data)
  themes/                themes installed by double-click or ashdash:// 
  url-trust.json         which workflows may be fired by ashdash://trigger
  run/ipc.sock, ipc.lock control socket and single-instance lock (or /tmp/ashdash-<uid>/ when the path is too long)
~/Library/Caches/ashdash/
  apps-index.json (versioned application index), icons/ (PNG cache, pruned to ~500 files), workflow-cache/ (alfred_workflow_cache), scripts/ (temporary script files, 0600)
  (the bookmark cache is in memory only)
```

## 7. Design decisions worth knowing

* **Coexist with the reference launcher**: own URL scheme, own data folder, own bundle id, different default hotkey.
* **egui kept for the window**, with native `NSWindow` styling applied afterwards (vibrancy, level, collection behaviour). An
  `NSPanel` rewrite was considered and rejected for now: [spikes/ui-focus.md](spikes/ui-focus.md).
* **No network in the core.** Web searches and bookmarks open in the user's browser.
* **Compat first, safety where the reference launcher has none**: workflows keep the reference launcher's semantics, but with import limits, process-group
  timeouts, output caps and protected environment names ([WORKFLOWS.md](WORKFLOWS.md), [SECURITY.md](SECURITY.md)).
* **One brand file** for names, but a rename is more than one file: [CLEAN_ROOM.md](CLEAN_ROOM.md#where-the-name-lives).

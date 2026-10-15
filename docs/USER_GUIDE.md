# Ashdash user guide

Ashdash is a keyboard launcher for macOS: press a hotkey, type, press Return. It is pre-release software (version 0.1.0,
no published release yet) and "Ashdash" is a working name ([CLEAN_ROOM.md](CLEAN_ROOM.md)). It is independent of the reference launcher
application and not affiliated with its publisher.

How to read this guide: statements about the window are taken from the code and from [QA.md](QA.md); parts nobody has been
able to check on a real desktop are marked **unverified** and collected in [section 9](#9-known-unverified-and-missing).

Related documents: [SETTINGS.md](SETTINGS.md) (every config key), [WORKFLOWS.md](WORKFLOWS.md) (reference-compatible workflows),
[TROUBLESHOOTING.md](TROUBLESHOOTING.md), [AUTOMATION.md](AUTOMATION.md) (CLI, URLs, AppleScript), [ARCHITECTURE.md](ARCHITECTURE.md).

## 1. Install

There is no signed release yet. Today you build it yourself. Requirements: macOS 12 or newer (the default deployment target
of `packaging/build-app.sh`), Rust 1.88 or newer (the code uses let-chains; the authors build with 1.95) and the Xcode command line tools.

**Run from source** (fastest, good for trying it out):

```sh
git clone <repository-url> ashdash && cd ashdash
cargo run -p ashdash --release          # starts the launcher; the first build takes a few minutes
```

**Build the app bundle** (needed for "launch at login", the `ashdash://` URL scheme, AppleScript and double-clickable
`.alfredworkflow` files, because those need a bundle with an `Info.plist`):

```sh
packaging/build-app.sh                  # writes dist/Ashdash.app, ad-hoc signed, universal (arm64 + x86_64)
ARCHS=native packaging/build-app.sh     # one architecture, about twice as fast
packaging/make-dmg.sh                   # dist/Ashdash-<version>.dmg
cp -R dist/Ashdash.app /Applications/
```

An ad-hoc signed app is not trusted by Gatekeeper. If macOS refuses to open it: right-click the app, choose Open once, or run
`xattr -dr com.apple.quarantine /Applications/Ashdash.app` ([RELEASING.md](RELEASING.md)). The x86_64 half of the universal
binary and notarized builds are **unverified**. A Homebrew cask is rendered from `packaging/homebrew/ashdash.rb.template`
but no tap exists yet, so `brew install --cask` does not work yet (**unverified**).

Uninstall: quit Ashdash, delete the app, and delete `~/Library/Application Support/ashdash` and `~/Library/Caches/ashdash`
(`ashdash paths` prints the exact locations).

## 2. First run

1. Start Ashdash. It is a menu-bar app: no Dock icon, a status item in the menu bar (Show Ashdash, Reload, Preferences... (disabled until the preferences window exists), Quit Ashdash).
2. Press **Option+Shift+Space**. The launcher appears on the screen that has the mouse pointer.
3. Type, for example `saf` and press **Return** to open Safari. **Esc** clears the query, a second **Esc** closes the window.
   **Cmd+Q** quits Ashdash.
4. Choose your own hotkey in `config.json` (`ashdash config set hotkey cmd+shift+space`, see [SETTINGS.md](SETTINGS.md)).
   Settings are read at start-up: **restart Ashdash after changing one**. (`ashdash reload` only re-scans applications,
   workflows, snippets and workflow hotkeys.)
5. If the hotkey is already used by another app, the window still opens from the menu-bar item and `ashdash show`, and
   its first row says that the hotkey is unavailable.

There is no preferences window yet; settings are a JSON file edited by hand or with `ashdash config` ([SETTINGS.md](SETTINGS.md)).

### Launch at login

```sh
ashdash login-item enable | disable | status
```

This needs the app bundle (`Ashdash.app`, best in `/Applications`) and macOS 13 or newer; from a bare `cargo run` binary the command answers "unavailable: launch at login
needs the app bundle".

## 3. Permissions

macOS asks for permissions per app; Ashdash asks for none up front. Each one unlocks exactly the features below. The names are
those of **System Settings > Privacy & Security**. If you run the bare binary from a terminal instead of `Ashdash.app`, macOS
attributes the request to the terminal app (Terminal, iTerm...) and you grant it there.

| Permission | What needs it | Symptom without it | Fix |
|---|---|---|---|
| **Accessibility** | Auto-paste: pressing Return on a clipboard or snippet result sends Cmd+V to the frontmost app. Also the workflow-hotkey option "selection in macOS" (sends Cmd+C via System Events). | Return on a `clip`/`snip` row copies the text but does not paste it, and an error message says "Copied. Auto-paste needs the Accessibility permission: open System Settings > Privacy & Security > Accessibility, enable Ashdash, then try again." | Add and enable Ashdash (or your terminal) under Accessibility, then try again. |
| **Automation** (Apple Events) | Everything done through AppleScript: Log Out, Restart, Shut Down, Empty Trash, some Universal Actions (Finder / Mail), `>` Terminal commands (Terminal), workflow Run NSAppleScript and music objects (the target app). | An error row such as "Not authorized to send Apple events to System Events (-1743)" or a command that silently does nothing, once you clicked "Don't Allow" in the first-use dialog. | System Settings > Privacy & Security > Automation: enable the target app (System Events, Finder, Terminal, Mail, Music) under Ashdash. If the entry is missing, quit Ashdash and run the action again to be asked once more. |
| **Full Disk Access** | Reading Safari's bookmarks (`~/Library/Safari/Bookmarks.plist` is protected) for `bm`. | `bm` shows the row "safari bookmarks need Full Disk Access"; Return opens that settings pane. | Enable Ashdash under Full Disk Access, then **restart** Ashdash. Other browsers do not need it. |

Notes:

* The global hotkey itself needs no permission.
* Workflow notifications are posted through `osascript`; if none appear, look at System Settings > Notifications for the
  entry macOS lists for it (usually "Script Editor"). **Unverified** on a clean account.
* The exact prompt text and which app macOS names in each prompt were not captured on a clean account. Treat the table as the
  intended behaviour, and see [TROUBLESHOOTING.md](TROUBLESHOOTING.md).

## 4. Keyboard

Global: **Option+Shift+Space** (configurable) toggles the launcher. Workflows can add their own global hotkeys
([WORKFLOWS.md](WORKFLOWS.md)).

In the launcher window (from `crates/app/src/gui/keys.rs`):

| Key | Does |
|---|---|
| Up / Down, Ctrl+P / Ctrl+N | move the selection |
| PageUp / PageDown | move by one page (9 rows) |
| Return | run the selected row (a row that is dangerous asks first: press Return again to confirm) |
| Cmd+Return | the row's alternative action: reveal in Finder for files and folders; copy instead of paste for `clip` and `snip` rows |
| Other modifiers + Return | passed to workflows, whose items can define a different result per modifier (Alt, Ctrl, Shift, Cmd) |
| Cmd+1 ... Cmd+9 | run the 1st ... 9th visible row |
| Tab | complete the query with the row's autocomplete text (folders: `~/Documents/`, workflow items that define one) |
| Cmd+L | show the row's text (or its title) as full-screen Large Type; Esc, Return or a click closes it |
| Cmd+Y, or a lone tap of Shift | Quick Look of the selected file (`qlmanage -p`); Esc closes it |
| Esc | close Quick Look / Large Type, else cancel a pending confirmation, else clear the query, else hide the window |
| Cmd+Q | quit Ashdash |
| Mouse click on a row | same as Return (with the held modifiers) |

While a modifier is held, a row's subtitle can change to describe that alternative; this is **unverified on screen**.
There is no Cmd+C "copy result" binding yet and no Universal Actions panel on Right Arrow (use the `actions` keyword below).

## 5. What to type

The launcher asks its providers in a fixed order and the first *exclusive* one wins (a keyword, `>`, a path); otherwise
results from several providers are merged and ranked, and fallback web searches close the list. Keywords are case-insensitive
and must be followed by a space (`open report`), or stand alone to show a hint row.

| You type | You get | Provider id* |
|---|---|---|
| a workflow's keyword (`ip`, `weather ...`) | that workflow's results | `workflow` |
| `> ls -l` | a row "Run in Terminal: ls -l" (opens Terminal and runs it) | `shell` |
| `open name`, `find name`, `' name` | files by name (Spotlight), recent files rank higher | `files` |
| `in text` | files by contents (Spotlight full text) | `files` |
| `kind:pdf`, `kind:image`, `kind:folder`, `kind:md,rs` (anywhere in a file search) | narrow to a kind or extension; a name that is not a known kind is taken as an extension | `files` |
| `actions <text, URL or path>` | the Universal Actions for it. A file or folder that exists: Open, Open With..., Reveal in Finder, Copy File, Copy Path, Copy Name, Quick Look, Move to Trash, Compress, Email as Attachment (files only), Get Info. A URL adds Open URL. Any text: Search Google, Copy, Large Type | `actions` |
| `clip` or `clipboard`, then optional words | clipboard history; Return pastes (needs Accessibility), Cmd+Return only copies back. Text, images and file lists are kept. Empty `clip` lists everything with pinned text first | `clipboard` |
| `clip clear`, `clip clear all` | clear the history (all asks twice; `clear` keeps pinned entries) | `clipboard` |
| `snip`, then words | snippets by name, keyword or collection; Return pastes the expanded text, Cmd+Return copies it | `snippets` |
| `bm`, then words | browser bookmarks (bare `bm` lists everything alphabetically) | `bookmarks` |
| `g rust`, `ddg x`, `wiki x`, `youtube x`, `imdb x`, `maps x`, `bing x`, `amazon x`, `google x` | a web search in your default browser. Edit or add searches under `web_searches` | `web-keyword` |
| `~/Doc`, `/usr/lo` | browse the file system; Tab goes into a folder, Return opens, Cmd+Return reveals | `path` |
| `2+3*4`, `(1+2)^3`, `= sqrt(81)` | the result; Return copies it. Prefix `=` forces a calculation | `calc` |
| `saf`, `vsc`, `visual studio` | applications: prefix of a word, word initials, or substring, ranked by what you launched before for similar queries | `apps` |
| `lock`, `sleep`, `restart`, ... | system commands: Lock, Sleep, Sleep Displays, Log Out, Restart, Shut Down, Start Screen Saver, Empty Trash. Log Out, Restart, Shut Down and Empty Trash ask for a second Return | `system` |
| anything else | "Search Google / Amazon / Wikipedia for '...'" as the last rows (the searches marked `fallback`) | `web-fallback` |

\* The id is what you put in `providers_disabled` to switch a feature off, for example
`ashdash config set providers_disabled calc,system`.

Details worth knowing:

* **Calculator**: constants `pi e tau`,
  operators `+ - * / ^ %` (also `×`, `·` and `÷`). Functions: `sqrt cbrt sin cos tan asin acos atan ln log log10 log2 exp abs floor ceil round pow min max`. Very deep nesting is refused instead of crashing.
* **Learning**: choosing a result for a query makes it rank higher the next time. Usage is saved in `usage.json`
  (see [data and privacy](#7-your-data-and-privacy)). Workflow results, file search, path browsing and `actions` keep their own order and are not boosted.
* **Apps** come from `app_dirs` (`/Applications`, `/System/Applications`, ...). New and removed apps are picked up
  automatically (file system events), the index is cached on disk.
* **Files** search only inside `file_search.roots` and skip `file_search.excluded_*`. A search that runs into its time or
  size limit says "No files found yet" with the reason, instead of looking empty.
* **Snippets** live in `snippets.json` and in per-collection files in the `snippets/` folder next to it, or come from importing a
  `.alfredsnippets` package (double-click it, or open it with Ashdash). Placeholders: `{clipboard}`, `{clipboard:2}`, `{date}`,
  `{time}`, `{datetime}`, `{date +7D:yyyy-MM-dd}`, `{isodate}`, `{random:uuid}`, `{random:1..6}`, `{snippet:keyword}`, `{cursor}`.
  Typing a snippet keyword anywhere in macOS (system-wide expansion) is **not implemented**.
* **Bookmarks** are read from Safari, Chrome (stable, Beta, Canary), Chromium, Brave, Edge, Vivaldi, Opera, Arc and Firefox.
  Arc and Firefox are **unverified** on real profiles. Choose browsers with `bookmarks.enabled_sources`.

## 6. Themes and workflows

* A theme is an `.alfredappearance` file. Opening one with Ashdash copies it to `themes/` and selects it in `config.json`;
  `ashdash theme <file>` applies one to the running launcher (it only parses the file when Ashdash is not running); or set
  `theme` in the config to its path to use it at every start.
* Double-clicking works when Ashdash is the handler for the file. If the reference launcher is installed it stays the default opener: use
  right-click > Open With > Ashdash, or `open -a /Applications/Ashdash.app file.alfredworkflow`.
* Workflows (`.alfredworkflow` files made for the reference launcher): `ashdash import <file>`, or open the file with Ashdash (confirmation dialog). Only import
  workflows you trust, they can run arbitrary scripts: read [WORKFLOWS.md](WORKFLOWS.md) first.

## 7. Your data and privacy

Everything stays on your Mac; Ashdash makes no network connections of its own (web searches and bookmarks open in your browser;
workflows can do whatever their scripts do).

| Where | What | Notes |
|---|---|---|
| `~/Library/Application Support/ashdash/config.json` | your settings | mode 0600 |
| `.../usage.json` | which result you chose for which query, in clear text | mode 0600. Delete the file to forget. |
| `.../clipboard.json`, `.../clipboard-images/` | clipboard history (text, images up to 8 MiB each, file lists) | mode 0600/0700. Entries marked concealed, transient or auto-generated by the copying app (password managers) are never recorded; copies made in apps listed in `clipboard.ignored_apps` (1Password, Bitwarden, KeePassXC, Keychain Access, Passwords by default) are skipped. Everything else you copy is stored while Ashdash runs. There is no setting to stop recording (disabling the `clipboard` provider only hides the `clip` search): lower `clipboard.max_entries`, set `clipboard.max_age_days`, extend `clipboard.ignored_apps`, or wipe it with `clip clear all`. The concealed-type filter is **unverified** with a real password manager. |
| `.../snippets.json`, `.../snippets/` | your snippets | |
| `.../workflows/`, `workflow-config/`, `workflow-data/` | installed workflows, their saved settings and data | |
| `.../themes/`, `url-trust.json`, `run/` | themes, which workflows may be started by `ashdash://` links, the control socket | |
| `~/Library/Caches/ashdash/` | app index, icon cache, workflow caches, bookmark cache | safe to delete |

`ashdash paths` prints the locations. There is no setting yet to turn off usage learning (the core can; the switch is not wired
to the config).

## 8. Command line

`ashdash --help` is the reference. The common ones:

```sh
ashdash query "vsc"            # print ranked results without opening a window
ashdash show                   # open the launcher (starts Ashdash if needed)
ashdash search "g rust"        # open it with text typed
ashdash reload                 # re-scan apps, workflows, snippets, workflow hotkeys
ashdash import my.alfredworkflow
ashdash workflows              # list installed workflows
ashdash config keys            # every setting name
ashdash config set clipboard.max_age_days 14
ashdash quit
```

URL scheme, AppleScript and the socket protocol: [AUTOMATION.md](AUTOMATION.md).

## 9. Known unverified and missing

Unverified (nobody has been able to confirm these on real data or a clean machine):
real Cmd+V auto-paste and concealed-clipboard behaviour; Safari bookmarks under a real Full Disk Access denial; Arc and Firefox
bookmark parsers on real profiles; Finder trash scripts (only a read-only contract test ran); the subtitle swap while a
modifier is held; behaviour on several monitors and over full-screen apps; the x86_64 slice; notarization and Gatekeeper
(`brew install --cask` too); AppleScript through JXA with a path instead of the bundle id; whether the launcher draws above Quick
Look; Large Type text that does not fit is not scrollable. The canonical list is "Known unverified items" in
[STATUS.md](STATUS.md).

Not built yet: preferences window and live reload of `config.json`, Universal Actions panel on Right Arrow, an in-window workflow import
confirmation (double-clicking a package shows a native "Import workflow?" dialog; `ashdash import` does not ask), contacts, dictionary,
System Settings panes, browser history, system-wide snippet expansion, music controls of its own.

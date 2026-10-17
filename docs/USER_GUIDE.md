# Ashdash user guide

Ashdash is a keyboard launcher for macOS: press a hotkey, type, press Return. It is pre-release software (version 0.1.0,
no published release yet) and "Ashdash" is a working name ([CLEAN_ROOM.md](CLEAN_ROOM.md)). It is independent of the reference launcher
application and not affiliated with its publisher.

How to read this guide: statements about the window are taken from the code and from [QA.md](QA.md); parts nobody has been
able to check on a real desktop are marked **unverified** and collected in [section 10](#10-known-unverified-and-missing).

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

1. Start Ashdash. It is a menu-bar app: no Dock icon, a status item in the menu bar (Show Ashdash, Reload, Preferences..., Quit Ashdash).
2. Press **Option+Shift+Space**. The launcher appears on the screen that has the mouse pointer.
3. Type, for example `saf` and press **Return** to open Safari. **Esc** clears the query, a second **Esc** closes the window.
   **Cmd+Q** quits Ashdash.
4. Choose your own hotkey in Preferences (menu-bar item > Preferences..., or `ashdash prefs`; General > "Record new
   shortcut...") or with `ashdash config set hotkey cmd+shift+space` ([SETTINGS.md](SETTINGS.md)). Settings apply to the
   running launcher within about a second; there is no need to restart (only `app_dirs` needs one).
5. If the hotkey is already used by another app, the window still opens from the menu-bar item and `ashdash show`, and
   its first row says that the hotkey is unavailable.

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
| Up / Down, Ctrl+P / Ctrl+N | move the selection. Slower results never take it from you: if you moved the selection while a query was still running, the result you moved to stays highlighted when the final list arrives (found again by identity, not by row); if you did not move it, the new top result is highlighted |
| PageUp / PageDown | move by one page (9 rows) |
| Return | run the selected row: the one that was highlighted in the frame you saw, even if a result list arrives at that moment (if that row is gone from the new list, nothing runs and the footer says so; press Return again). Cmd+1...9 follow the same rule. A row that is dangerous (Log Out, Restart, Shut Down, Empty Trash, Move to Trash, clearing the clipboard history) asks first: press Return **again** to confirm. A Return that you hold down does not confirm by key repeat; release it and press it again. |
| Cmd+Return | the row's alternative action: reveal in Finder for files and folders; copy instead of paste for `clip` and `snip` rows |
| Other modifiers + Return | passed to workflows, whose items can define a different result per modifier (Alt, Ctrl, Shift, Cmd) |
| Cmd+1 ... Cmd+9 | run the 1st ... 9th visible row (same rule for confirmations: a fresh press) |
| Tab | complete the query with the row's autocomplete text (folders: `~/Documents/`, workflow items that define one) |
| Cmd+L | show the row's text (or its title) as full-screen Large Type; text that does not fit scrolls (wheel, trackpad, arrows, Page keys); Esc, Return or a click closes it |
| Cmd+Y, or a lone tap of Shift | Quick Look of the selected file (`qlmanage -p`); Esc closes it |
| Right Arrow (cursor at the end of the text) | open the **actions panel** for the selected file, folder, URL or text result, or for the whole buffer ([below](#universal-actions-panel-and-file-buffer)) |
| Left Arrow (cursor at the start), Esc | inside the panel: back one level |
| Option+Up / Option+Down | add the selected file or folder to the buffer / remove it |
| Esc | close Quick Look / Large Type, else cancel a pending confirmation, else clear the query (in the panel: its filter), else go back out of the panel, else hide the window |
| Cmd+Q | quit Ashdash |
| Mouse click on a row | same as Return (with the held modifiers) |

While a modifier is held, a row's subtitle can change to describe that alternative; this is **unverified on screen**.
There is no Cmd+C "copy result" binding yet.

### Universal Actions panel and file buffer

Right Arrow on a result that is a file, folder, URL or text opens a list of what you can do with it, in the same window:

* **File or folder:** Open, Open With... (one level deeper: Right Arrow on it lists applications), Reveal in Finder, Copy File, Copy
  Path, Copy Name, Quick Look, Move to Trash, Compress, Email as Attachment (files only), Get Info. A `.alfredworkflow` file
  gets **Install Workflow...** first (it opens the import prompt, see [section 6](#6-themes-and-workflows)).
* **URL:** Open URL and the text actions. **Text** (a calculator result, a snippet, a clipboard entry): Search Google, Copy,
  Large Type.
* Typing filters the list; Return runs the selected action through the same path as any other result, so Move to Trash
  names the full path and asks for a second, fresh Return. The panel is not offered for system commands, workflow results or
  `>` Terminal rows. Actions you run here are not learned for ranking.
* Left Arrow (cursor at the start) or Esc (once the filter is empty) goes back; hiding the launcher closes the panel.
* **File buffer:** Option+Up adds the selected file or folder to a buffer (only files and folders; the footer says
  "Buffer: N items - Right: actions - Alt+Down: remove"), Option+Down removes it. With a non-empty buffer, Right Arrow opens
  the actions for the **whole buffer** (Reveal in Finder, Copy, Copy Path, Copy Name, Compress, Email as Attachment, Move to Trash).
  The buffer is emptied whenever the launcher is hidden or shown again.

The `actions <text>` keyword (below) lists the same actions for text you type or paste.

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
  (see [data and privacy](#8-your-data-and-privacy)); you can switch it off or clear it in Preferences. Workflow results, file search, path browsing and `actions` keep their own order and are not boosted. Clipboard rows and Universal Actions are never learned (what you typed to find them may be a fragment of a secret).
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
* Workflows (`.alfredworkflow` files made for the reference launcher) are code that runs as you, so **every way of installing one asks
  first**. Ashdash opens the package once, shows what it says it is (name, author as written in the package, which is not
  verified; bundle id; version) and what it can do, and installs exactly that copy only after you confirm:
  * `ashdash import <file>`: prints the text and asks `Install this workflow? [y/N]`. Only `y`/`yes` installs. `--yes`
    skips the question (scripts and CI); without a terminal and without `--yes` it refuses instead of assuming yes.
  * Double-clicking or opening the file with Ashdash: a native dialog whose default button is Cancel.
  * In the launcher: drop the file on the window, or select it in a file search and press Right Arrow > "Install
    Workflow...". The text appears in the window; Return installs (not before 600 ms, and only a fresh press), Esc cancels.
  * Preferences > Workflows: "Choose workflow file..." or drop a file on the window; the same text with Install / Cancel buttons.
  What the prompt tells you (the "trust summary"):
  * **Runs code:** how many scripts (Run Script, Script Filter, Run NSAppleScript, script output) and which interpreters, and
    how many paste the typed text straight into the script source (then what you type can become code).
  * **Runs in Terminal:** each Terminal Command object, quoted.
  * **Writes files:** each Write File object with overwrite / append / prepend and its target. Targets that are absolute,
    `~`, contain `..` or a `{placeholder}` are flagged as "anywhere your account can"; relative ones are listed as the workflow's
    own data or cache folder. A Write File that targets the workflow's own installed folder gets its own line, "Can modify its own files"
    (Ashdash refuses to let a workflow rewrite its own `info.plist`, also through a link). (A script that appends to a shell start-up file is a way to persist, so this is listed even though
    it runs no script.)
  * **Replaces what is on your clipboard** (Copy to Clipboard objects).
  * **Opens URLs with the scheme:** the distinct URL schemes of Open URL objects (`{dynamic}` = decided by data at run time).
  * **Opens files chosen at run time:** an Open File object with an empty file field (it opens whatever arrives as the argument) or a
    `{placeholder}` path; that can be an app or script shipped inside the package.
  * **Launches an application:** the application an Open File or Open URL object hands its target to; "(shipped with the workflow)" means a
    path inside the workflow's own folder, i.e. code the package brings with it. **Opens files shipped with the workflow** lists relative files
    Open File / Launch Files open.
  * **Refers to files outside its folder** (absolute, `~` or `..` paths), and **external triggers** that other workflows or
    `ashdash://` links can start.
  * The line "Runs no scripts and no terminal commands" appears only for a workflow that does none of the above. Anything the
    summary cannot see (what a script does, URLs a Script Filter returns at run time) is not covered: read the scripts.
  * **Replace:** if a workflow with the same bundle id is installed, the title says "Replace workflow", the line
    `This replaces "<name>" <version> (installed).` appears, and the old folder is swapped out (restored if the swap fails).
    **Settings you saved for that bundle id are kept and the new package reads them**, which can include tokens and
    passwords: the prompt says so when such values exist. Remove the file `workflow-config/<bundle id>.json` first if you do not trust the new package.
  * Everything the package wrote is cleaned before it is shown (control and invisible characters, such as right-to-left
    overrides, are stripped; long text is cut).
* The prompt is **not** shown for workflows that do not go through an import: a folder you put into `workflows/` by hand, and
  every workflow in a `workflow_dirs` folder (including a setting added by editing `config.json`) run as soon as they are
  found, and their hotkeys are registered. See [WORKFLOWS.md](WORKFLOWS.md#trust-model).
* Read [WORKFLOWS.md](WORKFLOWS.md) before importing anything you did not write.

## 7. Preferences

Open it from the menu-bar item (Preferences...), with `ashdash prefs`, or with `open ashdash://prefs`. It is a separate
program (`ashdash prefs-window`) with its own Dock icon and Cmd+Tab entry, so it never affects the launcher's focus. Only one
window exists; opening it again brings it forward. Every change is checked, written to `config.json` only when valid, and applied by the
running launcher within about a second ([SETTINGS.md](SETTINGS.md)). If a change could not be applied (bad hotkey, broken
file, a web search URL that is not allowed) the window shows why and the launcher keeps the previous setting.

The window has a sidebar (Cmd+1 ... Cmd+6 switch pages; Cmd+K jumps to the "Go to a setting..." field, which filters the page list by name
and opens the first match on Return). A strip at the bottom always shows the settings reload (CONFIG), the number of PROBLEMS and whether the
launcher is running. It follows the system light/dark appearance.

| Page | What you can do |
|---|---|
| **General** (Cmd+1) | Record a new launcher shortcut (system shortcuts such as Cmd+Space are flagged), pick a theme or import an `.alfredappearance`, results per search, Launch at login (needs the app bundle), **Learning**: "Rank results by what I pick" and "Forget what I picked..."; a RELOAD card and the launcher's warnings |
| **Search** (Cmd+2) | Switch features (providers) on and off; file search folders and exclusions and hidden files; bookmark sources; web searches (add, edit, remove; the URL is validated) |
| **Clipboard** (Cmd+3) | Clipboard retention, largest image, and apps never to record |
| **Snippets** (Cmd+4) | Add, edit, delete and import `.alfredsnippets` |
| **Workflows** (Cmd+5) | List with enable/disable and Remove for workflows in Ashdash's own folder (workflows from `workflow_dirs` are listed read-only), import with the prompt above, "Configure..." for a workflow's settings sheet, "What can it do?" for its trust summary |
| **About** (Cmd+6) | Version, licence, data and settings locations with Reveal in Finder buttons |

You can drop `.alfredworkflow`, `.alfredappearance` and `.alfredsnippets` files on the window. "Forget what I picked..." clears
`usage.json` (through the running launcher if there is one). Turning learning **off** only stops recording and ranking;
the old `usage.json` stays on disk until you press Forget (or delete it).

## 8. Your data and privacy

Everything stays on your Mac; Ashdash makes no network connections of its own (web searches and bookmarks open in your browser;
workflows can do whatever their scripts do).

| Where | What | Notes |
|---|---|---|
| `~/Library/Application Support/ashdash/config.json` | your settings | mode 0600 |
| `.../usage.json` | which result you chose for which query, in clear text | mode 0600. Switch learning off with Preferences > General (or `learn_usage`); that does **not** delete the file. "Forget what I picked..." (or deleting the file) does. |
| `.../clipboard.json`, `.../clipboard-images/` | clipboard history (text, images up to 8 MiB each, file lists) | mode 0600/0700. Entries marked concealed, transient or auto-generated by the copying app (password managers) are never recorded; copies made in apps listed in `clipboard.ignored_apps` (1Password, Bitwarden, KeePassXC, Keychain Access, Passwords by default) are skipped. Everything else you copy is stored while Ashdash runs. There is no setting to stop recording (disabling the `clipboard` provider only hides the `clip` search): lower `clipboard.max_entries`, set `clipboard.max_age_days`, extend `clipboard.ignored_apps`, or wipe it with `clip clear all`. The concealed-type filter is **unverified** with a real password manager. |
| `.../snippets.json`, `.../snippets/` | your snippets | |
| `.../workflows/`, `workflow-config/`, `workflow-data/` | installed workflows, their saved settings and data | Saved settings are keyed by bundle id and survive removal; a package with the same bundle id reads them. |
| `.../import-staging/` | the private copy of a package while its import prompt is open | mode 0700; deleted when you answer; leftovers older than a day are swept at the next import. Hidden `.import-*` folders in `workflows/` are the same kind of leftover (an install interrupted midway); they are never loaded, and are cleaned up (or the previous version put back) after an hour |
| `.../themes/`, `url-trust.json`, `run/` | themes, which workflows may be started by `ashdash://` links, the control socket, locks and the reload status the preferences window shows | |
| `~/Library/Caches/ashdash/` | app index, icon cache, workflow caches, bookmark cache | safe to delete |

`ashdash paths` prints the locations.

## 9. Command line

`ashdash --help` is the reference. The common ones:

```sh
ashdash query "vsc"            # print ranked results without opening a window
ashdash show                   # open the launcher (starts Ashdash if needed)
ashdash search "g rust"        # open it with text typed
ashdash reload                 # re-scan apps, workflows, snippets, workflow hotkeys
ashdash import my.alfredworkflow   # shows what it can do and asks; --yes skips the question
ashdash prefs                  # preferences window
ashdash workflows              # list installed workflows
ashdash config keys            # every setting name
ashdash config set clipboard.max_age_days 14
ashdash quit
```

URL scheme, AppleScript and the socket protocol: [AUTOMATION.md](AUTOMATION.md).

## 10. Known unverified and missing

Unverified (nobody has been able to confirm these on real data or a clean machine):
real Cmd+V auto-paste and concealed-clipboard behaviour; Safari bookmarks under a real Full Disk Access denial; Arc and Firefox
bookmark parsers on real profiles; Finder trash scripts (only a read-only contract test ran); the subtitle swap while a
modifier is held; behaviour on several monitors and over full-screen apps; the x86_64 slice; notarization and Gatekeeper
(`brew install --cask` too); AppleScript through JXA with a path instead of the bundle id; whether the launcher draws above Quick
Look. Wave 3 added code whose logic is covered by tests but whose on-screen behaviour was only partly watched (the actions
panel, the in-window import prompt, the preferences window, live reload, key-repeat handling, Large Type scrolling); see
[QA.md](QA.md) section 3 for the checklist and [STATUS.md](STATUS.md) for what was and was not run on a real desktop. The
canonical list is "Known unverified items" in [STATUS.md](STATUS.md).

Not built yet: contacts, dictionary, System Settings panes, browser history, system-wide snippet expansion, music controls of
its own, a workflow editor, reference-style sync.

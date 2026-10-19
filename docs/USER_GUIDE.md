# Ashdash user guide

Ashdash is a keyboard launcher for macOS: press a hotkey, type, press Return. It is pre-release software (version 0.1.0,
no stable release yet).

How to read this guide: statements about the window are taken from the code and from [QA.md](QA.md); parts nobody has been
able to check on a real desktop are marked **unverified** and collected in [section 10](#10-known-unverified-and-missing).

Related documents: [SETTINGS.md](SETTINGS.md) (every config key), [WORKFLOWS.md](WORKFLOWS.md) (workflows, file formats),
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
workflow packages, because those need a bundle with an `Info.plist`):

```sh
packaging/build-app.sh                  # writes dist/Ashdash.app, ad-hoc signed, universal (arm64 + x86_64)
ARCHS=native packaging/build-app.sh     # one architecture, about twice as fast
packaging/make-dmg.sh                   # dist/Ashdash-<version>.dmg
cp -R dist/Ashdash.app /Applications/
```

An ad-hoc signed app is not trusted by Gatekeeper. If macOS refuses to open it (on macOS 15 and later the dialog only offers *Done* and *Move to Trash*): open System Settings > Privacy & Security, press *Open Anyway* next to the
blocked app and confirm in the second dialog, or run `xattr -dr com.apple.quarantine /Applications/Ashdash.app` ([RELEASING.md](RELEASING.md)); right-click > Open is the route up to macOS 14. The x86_64 half of the universal
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
  Path, Copy Name, Quick Look, Move to Trash, Compress, Email as Attachment (files only), Get Info. A workflow package
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
| anything else | "Search Google / Amazon / Wikipedia for “...”" as the last rows (the searches marked `fallback`), set apart from real hits by a hairline; the URL shows when the row is selected | `web-fallback` |

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
  snippet package (double-click it, or open it with Ashdash). Placeholders: `{clipboard}`, `{clipboard:2}`, `{date}`,
  `{time}`, `{datetime}`, `{date +7D:yyyy-MM-dd}`, `{isodate}`, `{random:uuid}`, `{random:1..6}`, `{snippet:keyword}`, `{cursor}`.
  Typing a snippet keyword anywhere in macOS (system-wide expansion) is **not implemented**.
* **Bookmarks** are read from Safari, Chrome (stable, Beta, Canary), Chromium, Brave, Edge, Vivaldi, Opera, Arc and Firefox.
  Arc and Firefox are **unverified** on real profiles. Choose browsers with `bookmarks.enabled_sources`.

## 6. Themes and workflows

* A theme is an appearance file ([WORKFLOWS.md](WORKFLOWS.md#file-formats-and-identifiers)). Opening one with Ashdash copies it to `themes/` and selects it in `config.json`;
  `ashdash theme <file>` applies one to the running launcher (it only parses the file when Ashdash is not running); or set
  `theme` in the config to its path to use it at every start.
* **The look without a theme.** The launcher is a square, near-black panel (575 pt wide, 62 pt tall while the query is empty) with a thin
  dark-grey border and nothing in it but a large white text caret: no placeholder text, no icon. Results appear below the query line
  inside the same panel, which grows downward. The numbers are in [design/launcher-look.md](design/launcher-look.md). A theme file
  overrides every value (colours, `roundness`, `width`, text sizes). Besides the usual keys it may set `window` > `borderWidth`
  (points; without it the border is one device pixel, thin on every screen; `0` = no border) and `search` > `caretColor` /
  `caretWidth` (points). Files without them keep the defaults. The query font is the launcher's built-in sans-serif at the theme's `search` > `text` > `size` (34 pt by default).
* Double-clicking works when Ashdash is the handler for the file. If another application already opens those files it stays the default: use
  right-click > Open With > Ashdash, or `open -a /Applications/Ashdash.app <file>`.
* Workflows (workflow packages) are code that runs as you, so **every way of installing one asks
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

The window has its own title bar (drag it to move the window), and a sidebar (Cmd+1 ... Cmd+7 switch pages; Cmd+K jumps to the
"Go to a setting..." field, which filters the page list by name and opens the first match on Return). A strip at the bottom always shows
the settings reload (CONFIG), the number of PROBLEMS and whether the launcher is running, plus DISK ACCESS when Safari bookmarks are on
and Full Disk Access is not granted. It follows the system light/dark appearance.

| Page | What you can do |
|---|---|
| **General** (Cmd+1) | The launcher shortcut is one box: click it (or focus it and press Return or Space), then press the new combination; Esc cancels, Backspace goes back to the default, and a combination without Cmd, Option or Ctrl is refused with the reason (system shortcuts such as Cmd+Space are flagged). Theme: a dropdown and "Import theme..." (you can also drop an appearance file on the window or run `ashdash theme <file>`). Results per search, Launch at login (needs the app bundle), **Learning**: "Rank results by what I pick" and "Forget usage...". A RELOAD card, the launcher's warnings and, if Safari bookmarks are on without Full Disk Access, a card "Full Disk Access not granted" with "Fix..." (Return) that opens the Permissions page |
| **Search** (Cmd+2) | Switch features (providers) on and off; file search folders and exclusions and hidden files; bookmark sources; web searches (add, edit, remove; the URL is validated) |
| **Clipboard** (Cmd+3) | Clipboard retention, largest image, and apps never to record |
| **Snippets** (Cmd+4) | Add, edit, delete and import snippet packages |
| **Workflows** (Cmd+5) | List with enable/disable and Remove for workflows in Ashdash's own folder (workflows from `workflow_dirs` are listed read-only), import with the prompt above, "Configure..." for a workflow's settings sheet, "What can it do?" for its trust summary |
| **Permissions** (Cmd+6) | What macOS has granted Ashdash, each with a status light and word (Granted / Not granted / Asked on first use / Unknown), what it is for, and "Open System Settings" for the right pane: **Accessibility** (auto-paste and keystrokes), **Automation** (System Events, Finder, Terminal) and **Full Disk Access** (Safari bookmarks). "Re-check" (Cmd+R) probes again; the page also re-checks when you come back to the window. Looking at it never makes macOS show a prompt. The answers are for the program that shows the window: started from `Ashdash.app` that is Ashdash, started from a terminal macOS counts the terminal. After granting Full Disk Access, quit and reopen Ashdash |
| **About** (Cmd+7) | Version, licence, data and settings locations with Reveal in Finder buttons |

You can drop workflow packages, appearance files and snippet packages on the window. "Forget what I picked..." clears
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
ashdash import <package>       # shows what it can do and asks; --yes skips the question
ashdash prefs                  # preferences window
ashdash workflows              # list installed workflows
ashdash config keys            # every setting name
ashdash config set clipboard.max_age_days 14
ashdash quit
```

URL scheme, AppleScript and the socket protocol: [AUTOMATION.md](AUTOMATION.md).

## 10. Known unverified and missing

Unverified (nobody has been able to confirm these on real data, a physical keyboard or a clean machine):

* Pasting and clipboard: real Cmd+V auto-paste, and the concealed-clipboard filter with a real password manager (the pasteboard side with the marker type was verified in a VM).
* Bookmarks: Safari under a real Full Disk Access denial; the Arc and Firefox parsers on real profiles.
* Scripting: Finder trash scripts (only a read-only contract test ran); AppleScript through JXA with a path instead of the bundle id.
* Window behaviour: the subtitle swap while a modifier is held; several monitors, Spaces and full-screen apps; whether the launcher draws above Quick Look; Large Type scrolling on screen.
* Interface paths: the actions panel and file buffer (including Move to Trash from the panel), the in-window import prompt with a real held Return, the Finder import dialog with a real file, every
  preferences page, the hotkey recorder, live hotkey rebind and theme apply, the file watcher with real editors, the status-item "Preferences..." entry and `ashdash://prefs` from the bundle, `rerun` and `runningsubtext` on screen.
* The trust summary against real-world workflows (only synthetic ones and sample templates were used).
* Distribution: the x86_64 slice on Intel hardware; notarization and Gatekeeper with a real browser download; `brew install --cask`.

[QA.md](QA.md) section 3 is the checklist for all of these; section 4 lists the checks that need a physical Mac.

Known problems: none that are open. Two earlier ones are fixed and were checked in a macOS 15 VM from the packaged app:

* Chinese, Japanese and Korean text (typed, in result titles and in file names) used to be drawn as empty boxes. The launcher now loads a system font (Hiragino Sans, then PingFang or Hiragino Sans GB, and Apple SD Gothic Neo for Korean)
  the first time such a character is on screen, and nothing is loaded otherwise. The preferences window does the same for workflow, snippet and web search names and for text typed into its fields.
  Limits: the shapes of ideographs that Chinese and Japanese share follow the Japanese font; the lines of text are not vertically aligned perfectly when Latin and CJK mix; a Mac without any of these fonts still shows boxes.
* `ashdash login-item status` said "not found by macOS" on a fresh install. It says "off" until the first `enable`; the warning stays for a login item that was enabled and whose app was then moved or deleted.
  A login item enabled by an earlier version has no marker, so after moving the app it shows "off" instead of the warning; run `ashdash login-item enable` again.

Not built yet: contacts, dictionary, System Settings panes, browser history, system-wide snippet expansion, music controls of
its own, a workflow editor, sync.

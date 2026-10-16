# Settings reference

All settings live in one JSON file, `~/Library/Application Support/ashdash/config.json` (`ashdash config path` prints the
exact path; the file is created with mode 0600 the first time you save). A missing file means defaults. Edit it with an editor
or with the command line:

```sh
ashdash config keys                    # every setting name (dotted paths)
ashdash config get                     # the whole configuration as JSON (defaults included)
ashdash config get clipboard.max_entries
ashdash config set clipboard.max_age_days 14
ashdash config set providers_disabled calc,system
ashdash config set file_search.roots '["~/Documents","~/Projects"]'
```

* **Restart Ashdash after a change.** The running launcher reads the file once at start-up; `ashdash reload` re-scans
  applications, workflows, snippets and workflow hotkeys but does **not** re-read `config.json`. (Live reload is planned
  together with the preferences window.)
* `config set` takes a JSON value (`42`, `true`, `null`, `["a","b"]`, `"text"`); for a list setting a plain comma-separated
  text is also accepted (`calc,system`); other text is a string. Quote values that contain spaces. The change is checked
  by loading it back through the same parser as the file: a wrong type or an unknown key is refused and the file is not touched
  (`no setting 'nosuch'`). A file that cannot be parsed is never overwritten by `config set`.
* **Only the type is checked, not the meaning.** `ashdash config set hotkey 5` or `providers_disabled bogus` are accepted
  and then do nothing useful (an unusable hotkey shows a warning row in the launcher; unknown provider ids are ignored).
  Run `ashdash config get <key>` and test after changing.
* Keys that are lists or objects are leaves for `config set`, except you can address one element by index:
  `ashdash config set web_searches.0.fallback true`. `ashdash config keys` lists leaves only; for example `web_searches`
  is one key.
* A file written by a newer Ashdash (`version` greater than 2) is refused for saving, so settings you may not know about are
  not lost. Files without a `version` (older builds) are upgraded in memory (`clipboard_history_size` became
  `clipboard.max_entries`); the file is rewritten at the next save.
* Unreadable `config.json` at start-up: Ashdash prints `ashdash: ignoring invalid config.json: ...` on stderr and runs
  with defaults.
* **Paths:** `~` is expanded only in `file_search.*` paths. Write absolute paths in `app_dirs`, `workflow_dirs` and `theme`.

Defaults below are from `ashdash config get` on a fresh account (`<home>` is your home folder).

## Keys

| Key | Type | Default | What it does |
|---|---|---|---|
| `version` | number | `2` | Schema version of the file. Written by Ashdash; do not change. |
| `hotkey` | string | `"alt+shift+space"` | Global shortcut that shows or hides the launcher. See "Hotkey syntax". |
| `max_results` | whole number | `30` | Most rows a query returns (use 1 or more; 0 is accepted and then only the fallback web searches remain). The window shows 9 at a time and scrolls. File search also uses it as its limit. |
| `app_dirs` | list of paths | `/Applications`, `/System/Applications`, `/System/Library/CoreServices/Applications`, `/System/Library/CoreServices/Finder.app`, `<home>/Applications` | Where applications are looked for (3 folder levels deep; bundles inside bundles are not entered). Changes of these folders are noticed automatically. |
| `workflow_dirs` | list of paths | `[]` | Extra folders that contain workflow folders (each with an `info.plist`). Workflows you import with `ashdash import` go to Ashdash's own `workflows/` folder and are always loaded. |
| `web_searches` | list of objects | nine searches, see below | Keyword web searches. |
| `theme` | path or `null` | `null` (built-in dark theme) | An `.alfredappearance` file used at every start. |
| `providers_disabled` | list of strings | `[]` | Provider ids to switch off. |
| `bookmarks.enabled_sources` | list of strings | all eleven (below) | Which browsers `bm` reads, in order of precedence. |
| `bookmarks.include_in_general_search` | boolean | `true` | Also show up to 8 strong bookmark matches in ordinary queries (not only after `bm`). |
| `file_search.roots` | list of paths | `["~"]` | Search roots for `open`, `find`, `in`, `'`; results outside them are dropped. |
| `file_search.excluded_paths` | list of paths | `["~/Library"]` | Folders (and everything under them) never searched. |
| `file_search.excluded_names` | list of strings | `["node_modules","target"]` | Any path component with one of these names excludes the result. |
| `file_search.excluded_substrings` | list of strings | `["/pkg/mod/"]` | Results whose full path contains one of these are dropped. |
| `file_search.include_hidden` | boolean | `false` | Include dot-files and dot-folders. |
| `file_search.include_icloud_drive` | boolean | `true` | Keep iCloud Drive (`~/Library/Mobile Documents/com~apple~CloudDocs`) searchable although `~/Library` is excluded. |
| `clipboard.max_entries` | whole number | `200` | Most unpinned clipboard entries kept (use 1 or more). There is **no switch to stop recording**: the clipboard watcher always runs while Ashdash runs, and `providers_disabled` only removes the `clip` search. |
| `clipboard.max_age_days` | whole number or `null` | `null` | Drop unpinned entries older than this many days; `null` or `0` keeps them. |
| `clipboard.ignored_apps` | list of bundle ids | `com.1password.1password`, `com.agilebits.onepassword7`, `com.bitwarden.desktop`, `org.keepassxc.keepassxc`, `com.apple.keychainaccess`, `com.apple.Passwords` | Copies made while one of these apps is frontmost are never recorded. Setting this replaces the list, so include the defaults you want to keep. |
| `clipboard.max_image_bytes` | whole number (bytes) | `8388608` (8 MiB) | Copied images larger than this are not recorded. |

### Examples

```sh
ashdash config set hotkey cmd+shift+space
ashdash config set max_results 15
ashdash config set app_dirs '["/Applications","/System/Applications","/Users/me/Applications","/opt/homebrew/Caskroom"]'
ashdash config set workflow_dirs '["/Users/me/Library/Application Support/reference-launcher/reference-launcher.prefs/workflows"]'
ashdash config set theme /Users/me/Themes/Midnight.alfredappearance
ashdash config set theme null
ashdash config set providers_disabled clipboard,snippets
ashdash config set bookmarks.enabled_sources safari,firefox
ashdash config set bookmarks.include_in_general_search false
ashdash config set file_search.roots '["~/Documents","~/Projects"]'
ashdash config set file_search.excluded_names '["node_modules","target",".git"]'
ashdash config set clipboard.max_entries 50
ashdash config set clipboard.ignored_apps '["com.1password.1password","com.example.secretapp"]'
```

Pointing `workflow_dirs` at an existing reference launcher workflows folder runs those workflows in place. Ashdash only reads them there;
read [WORKFLOWS.md](WORKFLOWS.md) for what that means for trust.

## Hotkey syntax

`modifier+modifier+key`, case-insensitive, modifiers first, exactly one key last:

* modifiers: `alt` or `option`, `ctrl` or `control`, `shift`, `cmd`, `command` or `super`
  (`cmdorctrl` also works and means Command on macOS);
* keys: letters `a`..`z`, digits `0`..`9`, `space`, `enter`, `tab`, `escape`, `backspace`, `delete`, arrows (`arrowup` ...),
  `home`, `end`, `pageup`, `pagedown`, `f1`..`f24`, punctuation names such as `minus`, `equal`, `comma`, `period`, `slash`,
  `semicolon`, `quote`, `backquote`, `bracketleft`, `bracketright`, `backslash`; the parser is the `global-hotkey` crate's
  (`HotKey::from_str`).
* examples: `alt+shift+space` (default), `cmd+shift+space`, `ctrl+alt+k`, `f5`.
* `cmd+space` is Spotlight's shortcut; macOS keeps it unless you change Spotlight's.

If the string does not parse or the combination is taken, the launcher still starts: the first row says the hotkey is
unavailable, and `ashdash show` and the menu-bar item still work.

## `web_searches`

A list of objects:

| Field | Type | Meaning |
|---|---|---|
| `keyword` | string | What you type before the terms (`g rust`). Case-insensitive; must be followed by a space. |
| `title` | string | Name shown in the result row ("Search Google for ..."). |
| `url` | string | URL template; `{query}` is replaced by the percent-encoded terms. |
| `fallback` | boolean (optional, default `false`) | Also offered at the end of the list when nothing else matches. |

Default list: `google` (fallback), `g`, `ddg`, `bing`, `amazon` (fallback), `wiki` (fallback), `youtube`, `imdb`, `maps`.
Setting `web_searches` **replaces the whole list**, so start from `ashdash config get web_searches`. Example:

```sh
ashdash config set web_searches '[
 {"keyword":"g","title":"Google","url":"https://www.google.com/search?q={query}","fallback":true},
 {"keyword":"gh","title":"GitHub","url":"https://github.com/search?q={query}"}
]'
```

The template is not validated: a typo gives a broken link, and a URL that is not `http(s)` is opened by macOS with whatever
app handles that scheme.

## Provider ids (`providers_disabled`)

In precedence order: `workflow`, `shell` (`> cmd`), `files` (`open`, `find`, `in`, `'`), `actions`, `clipboard`, `snippets`,
`bookmarks`, `web-keyword`, `path` (`~/` and `/` browsing), `calc`, `apps`, `system`, `web-fallback`. A disabled provider is not
created at all. Unknown ids are ignored.

## Bookmark source ids

`safari`, `chrome`, `chrome-beta`, `chrome-canary`, `chromium`, `brave`, `edge`, `vivaldi`, `opera`, `arc`, `firefox`.
Unknown ids are ignored. Safari needs Full Disk Access ([USER_GUIDE.md](USER_GUIDE.md#3-permissions)); Arc and Firefox are unverified
on real profiles.

## Not in `config.json`

* **Workflow settings** (a workflow's own configuration sheet) are stored per workflow in `workflow-config/<bundle id>.json`
  next to `config.json`, mode 0600, as `{"variable": "value"}`. There is no command-line editor for them yet; a settings window
  is planned.
* **Snippets**: `snippets.json` (a flat list of `{"name","keyword","text"}`) and one file per collection in the `snippets/` folder;
  import `.alfredsnippets` packages by opening them. See [USER_GUIDE.md](USER_GUIDE.md#5-what-to-type).
* **Usage learning** (`usage.json`): cannot be switched off from the configuration yet. Delete the file to forget.
* **File search limits** (`mdfind` 4 s and 8 MiB, directory-walk fallback 2 s and 200 000 entries) are fixed in the code.
* **Which workflows are enabled**: a flag inside each workflow's `info.plist` (`disabled`); `ashdash workflows` shows it.

## Environment variables

| Variable | Effect |
|---|---|
| `ASHDASH_CONFIRM=1` | `ashdash run` performs a result that needs confirmation (Log Out, Shut Down, Empty Trash, clearing clipboard history) instead of only printing the question. |
| `ASHDASH_TRACE=1` | The launcher prints timestamped window and focus events to stderr (diagnostics). |
| `ASHDASH_WORKFLOW_DEBUG=1` | Output of workflow Debug objects goes to stderr. |
| `HOME` | Decides where `Library/Application Support/ashdash` is; a temporary `HOME` gives you a throw-away profile for testing. |

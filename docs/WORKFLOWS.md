# Workflows

Ashdash runs workflows in the format used by the reference launcher: a folder with an `info.plist` that describes a graph of **objects**
connected by **wires**, packaged as a zip with the extension `.alfredworkflow`. This page says which objects work, how to
install workflows, what they are allowed to do, and what to be careful about.

Ashdash is independent software; the format was learned from public documentation and from observing files
(see [CLEAN_ROOM.md](CLEAN_ROOM.md) and [REVERSE_ENGINEERING.md](REVERSE_ENGINEERING.md), which also says which details
are *inferred* rather than confirmed). Compatibility is a goal, not a promise: always test a workflow you depend on.

## Install, list, run

```sh
ashdash import ~/Downloads/some.alfredworkflow    # shows what it can do, asks, then installs into <data>/workflows/user.workflow.<bundle id>
ashdash import --yes some.alfredworkflow          # same, without the question (scripts, CI)
ashdash workflows                                 # name  [bundle id]  (disabled)
ashdash query "hello bob"                         # see the results without a window
ashdash trigger com.example.hello go "text"       # fire an External Trigger object (see below)
```

* **Every import path asks first** (the import prompt, described under [Trust model](#trust-model)): `ashdash import`
  asks `Install this workflow? [y/N]` on the terminal (`--yes` skips it; with no terminal and no `--yes` it refuses);
  double-clicking or opening a `.alfredworkflow` with Ashdash shows a native dialog whose default button is Cancel; in the
  launcher you drop the file on the window or press Right Arrow > "Install Workflow..." on it in a file search (Return
  installs, Esc cancels); Preferences > Workflows has "Choose workflow file..." and drag and drop with Install / Cancel buttons.
  The package is opened **once** into a private staging copy; what you were shown is what gets installed, even if the file
  changes meanwhile. Saying no, or closing the prompt, deletes the copy. The extensions `.alfred3workflow`, `.alfred4workflow` and `.alfred5workflow` are
  accepted like `.alfredworkflow` when opened in Finder or from the launcher; the preferences window's drop zone only takes `.alfredworkflow`.
* **Re-importing a workflow with the same bundle id replaces the installed copy.** The prompt says so
  (`Replace workflow "<name>"?` and `This replaces "<name>" <version> (installed).`). The old folder is kept aside until the new one is
  in place and restored if the swap fails. Anything inside the old folder (files the workflow wrote there, your edits) is gone with
  it; the workflow's data and cache folders and its **saved settings** (`workflow-config/<bundle id>.json`) are not: they are
  keyed by bundle id, so the new package reads the old settings (API tokens, credentials) as its own. The prompt adds a
  warning line when saved values exist. The reference launcher behaves the same way, so that an update keeps your configuration.
* `workflow_dirs` in the config ([SETTINGS.md](SETTINGS.md)) can point at other folders of workflow folders, for example an
  existing reference launcher `workflows` directory. They run **in place**; Ashdash does not copy them. Use absolute paths.
  **These folders never go through the import prompt**: whatever workflow is found there is loaded, its keywords work, its workflow hotkeys are
  registered and its External Triggers become reachable. Adding a folder is a settings change (Preferences cannot do it; it is done
  in `config.json` or `ashdash config set workflow_dirs ...`), so see the warning under Trust model.
* **Changes are picked up by themselves.** Ashdash watches its `workflows/` folder, the `workflow_dirs` folders, saved workflow settings and
  snippets; edits to workflow files and new or removed workflow folders take effect within about a second (after a 300 ms quiet period),
  including their keywords and hotkeys. A change to `workflow_dirs` re-scans the new folder list in the background (the footer shows
  "Reloading..."). `ashdash reload` (or the menu-bar item Reload) forces a re-scan of applications, workflows and snippets.
* Enable and disable: Preferences > Workflows has a checkbox per workflow in Ashdash's own folder (it writes the `disabled` flag
  of the `info.plist`). A disabled workflow is listed (`ashdash workflows` shows `(disabled)`) but does not run, and its hotkeys are not registered.
  Workflows in `workflow_dirs` are read-only there: edit their plist.
* **Workflow hotkeys** (`trigger.hotkey` objects) are registered for each enabled workflow and run by **workflow identity** (the
  workflow's folder name), not by bundle id. A workflow without a bundle id works, and two copies of one bundle id in different
  `workflow_dirs` roots each fire their own hotkey. (Two workflow *folders with the same name* in different roots cannot be told
  apart: the second one's hotkeys are refused with a warning.) A key that is already taken (by the launcher or an earlier workflow) is a warning on
  stderr and the first claim wins. Hotkeys follow the live reload: add or remove a workflow or a `workflow_dirs` folder and the
  registrations are redone; changing the launcher hotkey makes the old key available to workflows and reserves the new one.
* Per-workflow settings (the configuration sheet a workflow defines through `userconfigurationconfig`) become variables with
  their defaults. Edit them in Preferences > Workflows > "Configure..." (a value per entry and "Use default"); they are stored in
  `<data>/workflow-config/<bundle id>.json` (`{"variable": "value"}`, mode 0600), which you can also edit by hand.
* Remove: Preferences > Workflows > "Remove..." deletes a workflow from Ashdash's own folder (it asks again). Its saved settings stay on disk.

## A minimal workflow (verified with this build)

`info.plist` inside a folder, zipped to `hello.alfredworkflow` (`zip hello.alfredworkflow info.plist`):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>bundleid</key><string>com.example.hello</string>
  <key>name</key><string>Hello</string>
  <key>version</key><string>1.0</string>
  <key>objects</key>
  <array>
    <dict>
      <key>uid</key><string>KW</string>
      <key>type</key><string>alfred.workflow.input.scriptfilter</string>
      <key>version</key><integer>3</integer>
      <key>config</key>
      <dict>
        <key>keyword</key><string>hello</string>
        <key>withspace</key><true/>
        <key>argumenttype</key><integer>1</integer>   <!-- 0 required, 1 optional, 2 none -->
        <key>title</key><string>Say hello</string>
        <key>subtext</key><string>type a name</string>
        <key>type</key><integer>0</integer>            <!-- script language: 0 = bash -->
        <key>scriptargtype</key><integer>1</integer>   <!-- 1 = the query arrives as $1 -->
        <key>script</key>
        <string>printf '{"items":[{"title":"Hello %s","arg":"%s","variables":{"who":"%s"}}]}' "$1" "$1" "$1"</string>
      </dict>
    </dict>
    <dict>
      <key>uid</key><string>OUT</string>
      <key>type</key><string>alfred.workflow.output.largetype</string>
      <key>version</key><integer>1</integer>
      <key>config</key>
      <dict><key>largetypetext</key><string>Hi {var:who} ({query})</string></dict>
    </dict>
  </array>
  <key>connections</key>
  <dict>
    <key>KW</key>
    <array>
      <dict>
        <key>destinationuid</key><string>OUT</string>
        <key>modifiers</key><integer>0</integer>
        <key>vitoclose</key><false/>
      </dict>
    </array>
  </dict>
</dict>
</plist>
```

With that imported, `ashdash query "hello bob"` lists "Hello bob" and `ashdash run "hello bob"` prints `Hi bob (bob)`
(Large Type text goes to stdout on the command line).

## Compatibility by object type

Legend: **works** = implemented and covered by a test with a synthetic workflow; **partial** = works with the limits
stated; **no** = not run (the object is skipped, the stream stops there with a warning where it matters).
"Confirmed" and "inferred" refer to [REVERSE_ENGINEERING.md](REVERSE_ENGINEERING.md).

### Inputs and triggers

| Object (`alfred.workflow.…`) | State | Notes |
|---|---|---|
| `input.keyword` | works | `withspace`, argument required / optional / none, title and subtext. |
| `input.scriptfilter` | works | JSON and the legacy XML output. Supported: `items` with `uid title subtitle arg valid autocomplete icon text (copy, largetype) match type:file mods variables`; script-level `variables`; `rerun` (0.1 to 5 s: the launcher repeats the query while the text is unchanged and stops repeating 5 minutes after the first run; typing starts afresh); `cache` with `seconds` (clamped 5 s to 24 h) and `loosereload`; `skipknowledge` (accepted: workflow results keep the script's order anyway); `runningsubtext` (shown as the first rows while a slow script runs); `alfredfiltersresults` (the launcher filters by title / `match`). The XML `quicklookurl` is parsed but Quick Look does not use it. Time limit 10 s. Variable precedence: workflow < script-level < item < modifier. Script languages: 0 bash, 5 zsh (confirmed); 1 php, 2 ruby, 3 python3, 4 perl, 6 AppleScript, 7 JavaScript (JXA) (inferred). The `scriptargtype` modes: 0 `{query}` pasted into the script, 1 argv, 2 stdin (inferred). External script files are supported. |
| `input.listfilter` | works | Static list with fuzzy filtering. |
| `input.filefilter` | partial | Runs a Spotlight query from `fields`, `types`, `scopes`; `anchorfields` and `daterange` are read but not applied. How the pieces combine is inferred. The search is stopped by the file-search budget (`file_search.mdfind_timeout_ms`, `file_search.mdfind_max_bytes`); at most 200 hits are read. |
| `trigger.external` | works | Started by `ashdash trigger`, `ashdash://trigger` (after per-workflow consent) and AppleScript `run trigger`, and by Call External Trigger inside the same workflow. Carries an argument and variables. |
| `trigger.hotkey` | works | Global hotkey per workflow, argument none / selection / clipboard, "show launcher" or pass through, related-apps filter, focused-app variable. Not supported: non-zero `modsmode`, `leftcursor`. The key encoding is inferred. "Selection in macOS" uses Cmd+C via System Events, briefly replaces your clipboard and needs the Accessibility permission. Details: [AUTOMATION.md](AUTOMATION.md). |
| other triggers (snippet, universal action, fallback, contact, ...) | no | Not run. |

### Actions

| Object | State | Notes |
|---|---|---|
| `action.script`, `output.script` | works | bash / zsh / php / ruby / python3 / perl / osascript / JXA or a script file. 60 s limit, output capped at 8 MiB, the whole process group is killed on timeout. `{query}` escaping mask: bits 2, 4, 32, 64 (backquote, double quote, dollar, backslash) inferred; 1, 8, 16 (brackets, semicolon, space) unverified. |
| `action.applescript` | works | Runs through `osascript`. Needs Automation for the app it controls. |
| `action.openurl` | works | `utf8` and `spaces` options, `{query}` and `{var:…}`; optional browser. Any URL scheme is opened: see security. |
| `action.openfile`, `action.launchfiles`, `action.revealfile` | works | `~` is expanded; an empty file field means the incoming argument; a relative path (file, `openwith`, `browser`) is resolved against the workflow's own folder. |
| `action.terminalcommand` | works | Runs in Terminal. |
| `action.itunescommand` | partial | Play/pause, next, previous, volume, mute, rating, back track for Music.app (codes 1-3, 5-15); other codes warn. |
| `action.systemcommand` | partial | Only code 2 (Show Trash) is mapped. Other codes are deliberately not guessed, so a wrong guess can never log you out; they emit a warning. |
| `action.systemwebsearch` | partial | Only the default engine (Google); others warn. |
| `alfred.workflow.action.browseinalfred`, `alfred.workflow.action.actioninalfred` | partial | Both only show the launcher with the object's path or the incoming argument typed in ("show the launcher with this text"); the reference launcher's file browser and action list are not separate modes here. |

### Outputs

| Object | State | Notes |
|---|---|---|
| `output.notification` | works | Posted through `osascript`. |
| `output.clipboard` | works | Copies the text. |
| `output.largetype` | works | Full-screen overlay. |
| `output.writefile` | works | Overwrite / append / prepend, `adduuid`, intermediate folders; relative paths resolve into the workflow's data / cache folder or the workflow folder. Absolute and `~` paths can name any file you can write, so the import prompt lists every Write File target. Passes the resolved path on. |
| `output.callexternaltrigger` | partial | Same workflow only. Calling a trigger of another workflow warns and stops. |

### Utilities

| Object | State | Notes |
|---|---|---|
| `utility.argument`, `utility.junction`, `utility.debug` | works | Debug prints to stderr when `ASHDASH_WORKFLOW_DEBUG` is set. |
| `utility.conditional` | works | Several conditions with separate wires and an else wire; first match wins. Match modes: 0 equals is confirmed; 1 not equal, 2 greater, 3 less (as integers), 4 regex are **inferred** from menu order. Regex uses the Rust `regex` syntax: look-around and back-references fail to compile and the condition does not match (a warning is reported). |
| `utility.filter` | works | Equal / not equal / regex (legacy Filter). |
| `utility.replace` | works | String, regex and empty-input modes (inferred). |
| `utility.transform` | works | trim, upper, lower, capitalize, reverse, strip diacritics, strip non-alphanumeric (numbering inferred). |
| `utility.split` / `splitargument`, `utility.merge` / `joinargs` / `joinarguments` | works | Split into variables or into a list that a later Merge joins. |
| `utility.delay` | works | Continues later; the other streams run first. |

Anything not in the tables (for example a type introduced by a later reference launcher version) is not run. `Workflow::unsupported_kinds()`
reports them; no window shows that report yet.

Stream order matches the reference launcher's documented behaviour (outgoing wires run top to bottom by position, a Delay yields). A run
stops after 256 object executions, which protects against wire cycles.

### Variables and environment

Scripts get `alfred_workflow_name`, `alfred_workflow_bundleid`, `alfred_workflow_uid`, `alfred_workflow_version`,
`alfred_workflow_data`, `alfred_workflow_cache` (folders under Ashdash's own data and cache folders) and `alfred_version`
(always `5.0`, for compatibility), plus every workflow variable. `{query}`, `{var:name}` and `{allvars}` work in templates.
Not provided: `alfred_theme*`, `alfred_preferences`, `alfred_debug` and the other `alfred_*` variables of the reference launcher itself.

## Trust model

**A workflow is a program.** Importing one is the same decision as downloading and running a script: Ashdash does not
sandbox it. Scripts run as you, with your environment (SEC-14: they inherit the launcher's variables, including anything
sensitive in your shell profile if you started Ashdash from a terminal), can read your files, use the network and delete things.

What Ashdash does for you:

* Import is hardened (SEC-07, SEC-10, SEC-11): at most 2,000 entries and 100 MiB expanded, a ratio limit for bombs, names
  are confined to the workflow folder (`../`, absolute paths and escaping symlinks reject the whole package), the install
  is atomic.
* A script cannot run forever or fill memory: 10 s for Script Filters, 60 s for actions, 8 MiB of output, process group
  killed on timeout (SEC-05, SEC-06, SEC-16).
* Variables that come from data (Script Filter output, an earlier script) cannot set `PATH`, `HOME`, loader or interpreter
  hooks (`DYLD_*`, `BASH_ENV`, `PYTHONPATH`, `NODE_OPTIONS`, ...) for later scripts, and cannot define `alfred_*`
  (SEC-02). Variables the author sets statically in the plist still work. This is a deliberate difference from the reference launcher.
* `{query}` in `osascript` / Terminal command arguments is passed as data, never parsed as options (SEC-01).
* **The import prompt.** Before anything is installed, the package is opened once and described by `Workflow::trust_summary()`;
  the prompt shows the name, the author as the package states it (never verified), the bundle id, the version and description,
  and then, one line each:
  * the number of scripts and their interpreters (Run Script, Script Filter, Run NSAppleScript, script output) and how many paste the
    typed text into the script source;
  * every **Terminal command**;
  * every **Write File** target (overwrite / append / prepend, the path or `{dynamic}`, and whether it is confined to the
    workflow's own data/cache folder, its own installed folder (a separate line: "Can modify its own files"; the executor never lets Write File change the workflow's own `info.plist`, not even through a symlink or hard link), or can be anywhere your account can write: a shell start-up file or a LaunchAgent plist is a way to persist);
  * whether it **replaces what is on your clipboard** (Copy to Clipboard objects);
  * the **URL schemes** of its Open URL objects (`{dynamic}` when the scheme is decided by data);
  * **files outside its folder** it refers to (literal absolute, `~` or `..` paths in Open File, Launch Files and script files);
  * **files opened at run time**: an Open File object with an empty file field (it opens the incoming argument) or a path with a `{placeholder}`, which can be an app or script shipped inside the package;
  * **applications launched**: the application of an Open File (`openwith`) or Open URL / web search (`browser`) object, marked "shipped with the workflow" when it is a path inside the workflow's own folder, and **files shipped with the workflow** that Open File / Launch Files open (a relative path is resolved against the workflow's folder);
  * its **External Triggers** (what another workflow or an `ashdash://` link can start);
  * a replacement notice and the saved-settings warning when the bundle id is already installed (see above).
  "Runs no scripts and no terminal commands." is printed only when none of the above applies (`TrustSummary::is_passive()`); a workflow that
  runs no code but does something else gets "Runs no scripts or Terminal commands, but it can do the following:" followed by the lines above
  (checked with a package whose only object is a Write File that appends to `~/.zshenv`: the prompt says "Writes files anywhere your account can ... appends to ~/.zshenv").
  Reveal in Finder is deliberately not listed (it only selects a file). The text is built from the package's `info.plist`, with
  control characters and invisible Unicode formatting (right-to-left overrides, zero-width characters) removed and long
  values cut, so a package cannot disguise the prompt. Returning is guarded: the launcher prompt ignores Return for 600 ms
  and a held-down Return never confirms by repeating.
  What it cannot know: what a script actually does, the URLs or files a Script Filter returns at run time, and what a bundled
  binary does. **The prompt is a description, not a sandbox or a scan.**
* `ashdash://trigger` can only fire an **External Trigger** of an **enabled** workflow, and only after you agreed once per
  workflow in a dialog (default: Don't Allow). There is no URL verb that imports, runs arbitrary scripts or touches files.

What it cannot do:

* It cannot tell a good script from a bad one. `{query}` pasted into script source (`scriptargtype` 0) lets whatever you type
  (or paste, or a web page puts on your clipboard) become shell code unless the script quotes it; the default escape mask
  leaves `; | & ' < >` and newline live (SEC-03, same as the reference launcher). Prefer workflows that use argv.
* Open URL objects open any scheme (`file:`, `ssh:`, custom handlers) with text that can come from a Script Filter (SEC-13).
* Workflow hotkeys are chosen by the workflow author. A hotkey workflow can run something every time you press that key.
* Workflows in `workflow_dirs` can be changed by whoever can write to that folder, at any time.
* **The import prompt is not the only way in.** It guards the install paths above. Putting a folder into `workflows/` by hand, or adding
  a path to `workflow_dirs` by editing `config.json` (or `ashdash config set workflow_dirs`), activates that folder's workflows, hotkeys
  and External Triggers with **no prompt**, and the running launcher applies the change live. A process that can write your
  `config.json` can already run code as you, so this is documentation, not a new privilege; but only point `workflow_dirs` at
  folders only you can write, and review a `workflow_dirs` entry you did not add. The `ashdash://` URL scheme, the control socket and AppleScript have no import verb and cannot add a folder.

Rules of thumb: import only from people you trust; read the scripts; keep Ashdash's data folder and your workflow folders writable only by you;
do not point `workflow_dirs` at a shared or downloaded folder. Current status of every finding: [SECURITY.md](SECURITY.md).

## Differences from the reference launcher worth knowing

* No workflow editor. Enable/disable, settings and removal exist in Preferences (for workflows in Ashdash's own folder).
* Browse in the reference launcher / Action in the reference launcher only show the launcher with a text typed.
* Environment, theme and preference variables of the reference launcher itself are not provided (see above).
* Variables from untrusted data cannot override a protected set of environment names (SEC-02).
* Workflows live in Ashdash's own folder; your reference launcher preferences are never touched. Sync is not supported.
* Regular expressions use Rust's `regex` crate, not ICU.

# Workflows

Ashdash runs workflows in the format used by the reference launcher: a folder with an `info.plist` that describes a graph of **objects**
connected by **wires**, packaged as a zip with the extension `.alfredworkflow`. This page says which objects work, how to
install workflows, what they are allowed to do, and what to be careful about.

Ashdash is independent software; the format was learned from public documentation and from observing files
(see [CLEAN_ROOM.md](CLEAN_ROOM.md) and [REVERSE_ENGINEERING.md](REVERSE_ENGINEERING.md), which also says which details
are *inferred* rather than confirmed). Compatibility is a goal, not a promise: always test a workflow you depend on.

## Install, list, run

```sh
ashdash import ~/Downloads/some.alfredworkflow    # installs into <data>/workflows/user.workflow.<bundle id>
ashdash workflows                                 # name  [bundle id]  (disabled)
ashdash query "hello bob"                         # see the results without a window
ashdash trigger com.example.hello go "text"       # fire an External Trigger object (see below)
```

* Double-clicking a `.alfredworkflow` file (with the app bundle installed) asks "Import the workflow ...?" in a native dialog
  whose default button is Cancel. `ashdash import` does **not** ask: it is the shell, you typed it.
* Re-importing a workflow with the same bundle id replaces the installed copy (the old one is kept aside until the new one is
  in place, and restored if the swap fails).
* `workflow_dirs` in the config ([SETTINGS.md](SETTINGS.md)) can point at other folders of workflow folders, for example an
  existing reference launcher `workflows` directory. They run **in place**; Ashdash does not copy them. Use absolute paths.
* After editing workflow files by hand, run `ashdash reload` (or the menu-bar item Reload).
* A workflow whose `info.plist` has `disabled` set is listed but does not run. There is no switch in Ashdash yet; edit
  the plist.
* Per-workflow settings (the configuration sheet a workflow defines through `userconfigurationconfig`) become variables with
  their defaults; values are read from `<data>/workflow-config/<bundle id>.json` (`{"variable": "value"}`, mode 0600). There is no
  editor in the UI yet; edit that file.

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
| `input.scriptfilter` | works | JSON and the legacy XML output. Supported: `items` with `uid title subtitle arg valid autocomplete icon text (copy, largetype) match type:file mods variables`; script-level `variables`; `rerun` (reported to the caller; the window does not re-run it yet); `cache` with `seconds` (clamped 5 s to 24 h) and `loosereload`; `skipknowledge` (accepted: workflow results keep the script's order anyway); `runningsubtext` (reported to the caller; the window does not show it yet); `alfredfiltersresults` (the launcher filters by title / `match`). The XML `quicklookurl` is parsed but Quick Look does not use it. Time limit 10 s. Variable precedence: workflow < script-level < item < modifier. Script languages: 0 bash, 5 zsh (confirmed); 1 php, 2 ruby, 3 python3, 4 perl, 6 AppleScript, 7 JavaScript (JXA) (inferred). The `scriptargtype` modes: 0 `{query}` pasted into the script, 1 argv, 2 stdin (inferred). External script files are supported. |
| `input.listfilter` | works | Static list with fuzzy filtering. |
| `input.filefilter` | partial | Runs a Spotlight query from `fields`, `types`, `scopes`; `anchorfields` and `daterange` are read but not applied. How the pieces combine is inferred. |
| `trigger.external` | works | Started by `ashdash trigger`, `ashdash://trigger` (after per-workflow consent) and AppleScript `run trigger`, and by Call External Trigger inside the same workflow. Carries an argument and variables. |
| `trigger.hotkey` | works | Global hotkey per workflow, argument none / selection / clipboard, "show launcher" or pass through, related-apps filter, focused-app variable. Not supported: non-zero `modsmode`, `leftcursor`. The key encoding is inferred. "Selection in macOS" uses Cmd+C via System Events, briefly replaces your clipboard and needs the Accessibility permission. Details: [AUTOMATION.md](AUTOMATION.md). |
| other triggers (snippet, universal action, fallback, contact, ...) | no | Not run. |

### Actions

| Object | State | Notes |
|---|---|---|
| `action.script`, `output.script` | works | bash / zsh / php / ruby / python3 / perl / osascript / JXA or a script file. 60 s limit, output capped at 8 MiB, the whole process group is killed on timeout. `{query}` escaping mask: bits 2, 4, 32, 64 (backquote, double quote, dollar, backslash) inferred; 1, 8, 16 (brackets, semicolon, space) unverified. |
| `action.applescript` | works | Runs through `osascript`. Needs Automation for the app it controls. |
| `action.openurl` | works | `utf8` and `spaces` options, `{query}` and `{var:…}`; optional browser. Any URL scheme is opened: see security. |
| `action.openfile`, `action.launchfiles`, `action.revealfile` | works | `~` is expanded; an empty file field means the incoming argument. |
| `action.terminalcommand` | works | Runs in Terminal. |
| `action.itunescommand` | partial | Play/pause, next, previous, volume, mute, rating, back track for Music.app (codes 1-3, 5-15); other codes warn. |
| `action.systemcommand` | partial | Only code 2 (Show Trash) is mapped. Other codes are deliberately not guessed, so a wrong guess can never log you out; they emit a warning. |
| `action.systemwebsearch` | partial | Only the default engine (Google); others warn. |
| `alfred.workflow.action.browseinalfred`, `alfred.workflow.action.actioninalfred` | partial | The workflow produces a "show the launcher with this text" request, but the window does not act on it yet: the run ends with an error message ("not wired up yet"). |

### Outputs

| Object | State | Notes |
|---|---|---|
| `output.notification` | works | Posted through `osascript`. |
| `output.clipboard` | works | Copies the text. |
| `output.largetype` | works | Full-screen overlay. |
| `output.writefile` | works | Overwrite / append / prepend, `adduuid`, intermediate folders; relative paths resolve into the workflow's data / cache folder or the workflow folder. Passes the resolved path on. |
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
reports them; the window does not show that report yet.

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
* The core can describe what a workflow will do before you import it: `Workflow::trust_summary()` lists the number of
  scripts and their interpreters, how many paste `{query}` into the script source, the URL schemes of Open URL objects,
  Terminal commands, files outside the workflow folder, and the External Triggers. **No window shows this yet**; the
  double-click dialog is a generic warning. Until then, open the `info.plist` and the scripts yourself before importing.
* `ashdash://trigger` can only fire an **External Trigger** of an **enabled** workflow, and only after you agreed once per
  workflow in a dialog (default: Don't Allow). There is no URL verb that imports, runs arbitrary scripts or touches files.

What it cannot do:

* It cannot tell a good script from a bad one. `{query}` pasted into script source (`scriptargtype` 0) lets whatever you type
  (or paste, or a web page puts on your clipboard) become shell code unless the script quotes it; the default escape mask
  leaves `; | & ' < >` and newline live (SEC-03, same as the reference launcher). Prefer workflows that use argv.
* Open URL objects open any scheme (`file:`, `ssh:`, custom handlers) with text that can come from a Script Filter (SEC-13).
* Workflow hotkeys are chosen by the workflow author. A hotkey workflow can run something every time you press that key.
* Workflows in `workflow_dirs` can be changed by whoever can write to that folder, at any time.

Rules of thumb: import only from people you trust; read the scripts; keep Ashdash's data folder and your workflow folders writable only by you;
do not point `workflow_dirs` at a shared or downloaded folder. Current status of every finding: [SECURITY.md](SECURITY.md).

## Differences from the reference launcher worth knowing

* No workflow editor, no workflow-level enable/disable or configuration window yet.
* The Script Filter `rerun` re-query and `runningsubtext` display, and the Browse / Action-in-the reference launcher outputs, need launcher
  window work that is not done.
* Environment, theme and preference variables of the reference launcher itself are not provided (see above).
* Variables from untrusted data cannot override a protected set of environment names (SEC-02).
* Workflows live in Ashdash's own folder; your reference launcher preferences are never touched. Sync is not supported.
* Regular expressions use Rust's `regex` crate, not ICU.

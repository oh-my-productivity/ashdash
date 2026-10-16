# Automation surface (WP9)

Everything that lets something other than the keyboard drive a running Ashdash. All entry points end in one
`AppEvent` channel (`crates/app/src/events.rs`) that the window drains every frame, or in `Engine::run_trigger`.

| Entry point | Code | Trust |
|---|---|---|
| CLI → unix socket | `ipc.rs`, `main.rs` | same uid (`getpeereid`), like the shell user |
| `ashdash://` URLs | `urlscheme.rs` | **untrusted**: allow-list, rate limit, per-workflow consent |
| AppleScript / JXA | `scripting.rs`, `resources/Ashdash.sdef` | macOS Automation (TCC) consent |
| Workflow hotkeys | `hotkeys.rs` | set by the workflow author; the reference launcher strips them on export |
| Preferences window | `gui/prefs/` | a separate process that talks to the launcher only through files in the data folder (and `clear-usage` on the socket) |

## CLI and socket

`ashdash` (no args) starts the launcher. A flock on `<data>/run/ipc.lock` makes it single-instance; a second
`ashdash` sends `show` to the first. The socket is `<data>/run/ipc.sock` (dir 0700, socket 0600); when that path would exceed
`sun_path` (104 bytes) it is `/tmp/ashdash-<uid>/ipc.sock` (same owner/mode checks, symlinks refused).
Protocol: one JSON line per request, one per answer.

```
{"cmd":"ping"|"show"|"reload"|"quit"|"prefs"|"clear-usage"}
{"cmd":"search","text":"saf"}
{"cmd":"trigger","bundle":"com.x","id":"go","arg":"..."}     -> large:true when the answer is Large Type text
{"cmd":"theme","file":"/abs/path.alfredappearance"}
```

CLI: `ashdash show | search <text> | trigger <bundle> <id> [arg] | reload | quit | theme <file> | prefs`. `show` and `search` start
the launcher when none runs; `trigger` runs in-process when none runs; `reload`/`quit` do nothing then; `theme` only
parses. `prefs` starts the preferences window (`ashdash prefs-window`, an internal command) as its own process whether or not the
launcher runs; the menu-bar item, `ashdash://prefs` and the socket verb `prefs` ask the running launcher, which starts it the same way. `clear-usage` is sent by the
preferences window's "Forget what I picked..." so the running launcher wipes its in-memory table and `usage.json`.
`ashdash import [--yes] <file>` shows the trust summary and asks (details in [WORKFLOWS.md](WORKFLOWS.md)), installs, and then tells
a running launcher to reload (new workflows, new hotkeys). The socket itself has **no import verb**.

## URL scheme `ashdash://` (never `reference://`)

```
ashdash://show
ashdash://search?query=<text>                       (<= 1024 bytes)
ashdash://trigger?bundle=<id>&name=<trigger id>&arg=<text>   (arg <= 4096 bytes)
ashdash://reload
ashdash://prefs                                     (no parameters; opens the preferences window)
ashdash://theme?name=<file name>      -> <data>/themes/<name>.alfredappearance, plain names only
```

Strict parser: fixed verbs and parameter names, no userinfo/port/path, no repeated or unknown keys, `%XX` decoded exactly once
(`+` stays `+`), UTF-8 only, no control characters (tab/newline allowed in `arg`), 16 KiB total. Rate limit: 6 URLs per 10 s.
`trigger` fires only an **External Trigger** object of an **enabled** workflow, and only after the user consented once per bundle id
(native dialog, default button "Don't Allow", remembered either way in `<data>/url-trust.json`, 0600; edit the file to
reset). Invocations are logged as verb + workflow, never arguments. There is no verb for importing, scripts, files, clipboard or system commands.

Delivery: `kAEGetURL` Apple Events through `NSAppleEventManager`, registered early (URL that launches the app) and again
after launch (AppKit installs its own handler during launch). Requires a bundle declaring `CFBundleURLTypes` (WP10).

## AppleScript / JXA

```applescript
tell application "Ashdash" to search "saf"
tell application "Ashdash" to run trigger "go" in workflow "com.x" with argument "hello"
tell application "Ashdash" to reload   -- / show
```
JXA: use the bundle id (`Application('dev.ashdash.launcher').search('saf')`); `Application('/path/Ashdash.app')` fails
with -1701 for custom commands (the path form works for properties only; cause not investigated).
The bundle needs `NSAppleScriptEnabled=YES` and `OSAScriptingDefinition=Ashdash.sdef` with the sdef in `Contents/Resources`.
`run trigger` returns Large Type text, raises a script error for failures, and answers an empty result when the workflow takes
longer than 1.5 s (it keeps running). The standard suite is included, so `quit` works.

## Workflow hotkeys

`alfred.workflow.trigger.hotkey` encoding, inferred from the templates in the reference launcher's framework resources (see the module header
of `hotkeys.rs` for the table and which parts are inference): `hotkey` = `kVK_*` code, `hotmod` = `NSEvent.ModifierFlags`
(F5 + option template = 96 / 0x880000), `hotkey=0 && hotmod=0` = unset, `action` 0 pass through / 1 show launcher with the argument,
`argument` 0 none / 1 selection / 2 clipboard, `relatedAppsMode` 0 all / 1 only / 2 except `relatedApps`,
`focusedappvariable` exports the frontmost bundle id (`focusedapp` when the name is empty).
Hotkeys share the launcher hotkey's `GlobalHotKeyManager` and are routed by `hotkeys::route` from the single process-wide handler.
Conflicts (with the launcher or an earlier workflow) and unusable keys are warnings on stderr; first claim wins.
Firing runs on a worker thread and starts the graph at the hotkey node with `Engine::run_node(workflow_id, node_uid, arg, vars)`.
The workflow is found by its **workflow id** (its folder name under `workflows/` or a `workflow_dirs` root), not by bundle id, so
workflows without a bundle id work and duplicate bundle ids cannot make one hotkey run the other copy; `run_node` records no usage
and is called only with node uids taken from an installed workflow's own hotkey objects (it is not reachable from the socket, the URL scheme or
AppleScript, which use `run_trigger`). Disabled workflows get no hotkeys.
Hotkeys are re-registered on menu-bar Reload, `ashdash reload`, after `ashdash import`, and by **live reload**: when a workflow
folder, a `workflow_dirs` entry or the launcher hotkey changes in `config.json`, the registry follows the folders and launcher hotkey
now in force. The `ashdash://trigger` consent check also reads the current `workflow_dirs`. Because that setting is watched, editing
`workflow_dirs` activates the hotkeys and External Triggers of the new folder **without the import prompt**
(see [SECURITY.md](SECURITY.md) section 3).

Known gaps: `modsmode != 0` hotkeys are skipped; "selection in macOS" uses Cmd+C via System Events and briefly replaces the
clipboard (the history watcher can see the probe and the selection); `leftcursor` is ignored.

# Troubleshooting

Start with the facts: what is the exact symptom, and what does Ashdash print on stderr? A GUI app started from Finder or
the Dock has no visible stderr, so for diagnosis run the binary from a terminal:

```sh
/Applications/Ashdash.app/Contents/MacOS/ashdash          # the bundle build
ASHDASH_TRACE=1 cargo run -p ashdash                      # from source, with window/focus tracing
```

Ashdash has no log file yet; everything goes to stderr. Messages begin with `ashdash:`.

Where things live: `ashdash paths` prints the data, config, workflow, snippet and cache locations. The commands below never
start the window: `ashdash query "<text>"`, `ashdash config get`, `ashdash workflows`, `ashdash paths`.

## The hotkey does nothing

1. Is Ashdash running? A menu-bar item (a magnifying glass) should be present. If not, start it. `ashdash show` opens the
   window and starts Ashdash if it was not running; if that works but the hotkey does not, the hotkey is the problem.
2. Open the launcher with `ashdash show`. If the first row says **"Hotkey ... is unavailable (...)"** then registration failed:
   * another app (or macOS: Spotlight uses Cmd+Space, input-source switching uses Ctrl+Space) already owns the combination,
     or
   * the `hotkey` string does not parse (`invalid hotkey: ...`), for example `hotkey = "5"` or two keys
     ("alt+a+b"). Fix it with `ashdash config set hotkey alt+shift+space` and restart; the syntax is in
     [SETTINGS.md](SETTINGS.md#hotkey-syntax).
3. You changed `hotkey` while Ashdash was running: it is re-registered within about a second. If the new key could not be registered (taken, or does not parse) the **old key stays active**, you get a notification and the preferences window (General page) says why. Choose another combination there; `ashdash prefs` opens it. A hotkey that was bad at start-up stays unusable until you fix the setting; no restart is needed after fixing it.
4. Two Ashdash processes: the second one only forwards "show" to the first (`ashdash: already running`). Check Activity
   Monitor for stray copies, for example a build from another folder.

## The window opens but typing goes elsewhere, or focus is not returned

The focus behaviour was measured on a development machine ([spikes/ui-focus.md](spikes/ui-focus.md)): the launcher took key
focus 20 of 20 times and gave it back to the previous app. Not covered: full-screen apps, several physical monitors and
secure-input fields (password boxes). If it happens to you, note the macOS
version, whether the front app was full-screen, and `ASHDASH_TRACE=1` output, and report it.

The launcher hides when it loses focus; that is intended. A Quick Look preview or Large Type is drawn over it;
whether the launcher draws above Quick Look is **unverified**.

## `ashdash show` / `search` / `reload` says something about a socket

* `ashdash is not running`: `reload` and `quit` only act on a running launcher. `show` and `search` start one.
* `the launcher did not start within 15 s`: start it from a terminal and read stderr.
* `ashdash: control socket unavailable: ...` at start-up: the launcher still works but cannot be driven by the CLI, the URL scheme
  or AppleScript. The socket is `<data>/run/ipc.sock`; when that path is too long for a Unix socket (104 bytes) Ashdash uses
  `/tmp/ashdash-<uid>/ipc.sock`. A stale file from a crash is replaced at the next start. Details: [AUTOMATION.md](AUTOMATION.md).

## Results are missing

| What is missing | Check |
|---|---|
| An application | Is its folder in `ashdash config get app_dirs`? (`app_dirs` is the one setting that needs a restart.) Only 3 folder levels deep are scanned and apps inside apps are skipped. `ashdash reload`. If the list looks stale, quit and delete `~/Library/Caches/ashdash` (the app index is rebuilt). |
| Files (`open x`) | Spotlight must index the folder: `mdutil -s /` (System Settings > Siri & Spotlight > Search Privacy lists excluded folders). Results outside `file_search.roots` or inside `excluded_paths` / `excluded_names` are dropped; `~/Library` is excluded by default. A row "No files found yet" means the search budget ran out (`file_search.mdfind_timeout_ms` 4 s and `mdfind_max_bytes` 8 MiB by default): refine the query, or raise the budget in [SETTINGS.md](SETTINGS.md). |
| Bookmarks (`bm`) | "Loading bookmarks..." just means the first read is still running; try again. A row "safari bookmarks need Full Disk Access": grant it and restart ([USER_GUIDE.md](USER_GUIDE.md#3-permissions)). Other browsers: is the browser's id in `bookmarks.enabled_sources`? Arc and Firefox are unverified on real profiles. |
| A web search | Is its keyword in `web_searches`? A keyword needs a space and text after it (`g rust`); alone it shows the normal list. A search whose URL is not `http(s)://host/...{query}` (no spaces, no user name, only `{query}` as placeholder) is **skipped** with a warning in the preferences window and on stderr. |
| A feature entirely (`clip`, `snip`, `calc`, ...) | `ashdash config get providers_disabled`: is the provider switched off? (Preferences > Features lists the same switches.) |
| Everything is empty except web searches | `max_results` is `0`. |
| Ranking does not learn | `learn_usage` is `false` (Preferences > General > "Rank results by what I pick"). Clipboard rows and Universal Actions are never learned. |
| Clipboard entries | Copies from an app in `clipboard.ignored_apps`, or marked concealed/transient by the app, are never recorded. Entries older than `clipboard.max_age_days` are dropped. The history is recorded only while Ashdash runs. |

## Pressing Return on a clipboard or snippet row does not paste

The row's text was copied, and the message says "Auto-paste needs the Accessibility permission". Add Ashdash (or the terminal you
started it from) in System Settings > Privacy & Security > Accessibility and try again. After granting, if it still fails, quit
and restart Ashdash: macOS caches the permission per process. Pasting works by sending Cmd+V; some apps and secure fields ignore
synthetic keys. **Unverified**: real Cmd+V behaviour in many apps.

To reset a permission that seems stuck (a standard macOS tool, not part of Ashdash):

```sh
tccutil reset Accessibility dev.ashdash.launcher
tccutil reset AppleEvents dev.ashdash.launcher
tccutil reset SystemPolicyAllFiles dev.ashdash.launcher     # Full Disk Access
```

Then start the action again so macOS asks again. If you run the bare binary instead of `Ashdash.app` the permission belongs to
your terminal app, not to `dev.ashdash.launcher`.

## "Not authorized to send Apple events" or an action does nothing

Log Out, Restart, Shut Down, Empty Trash, Terminal commands and some Universal Actions are AppleScript. macOS asks once per
target app (System Events, Finder, Terminal, Mail ...). If you clicked "Don't Allow", enable it under System Settings > Privacy &
Security > Automation > Ashdash, or reset with the `tccutil` line above. The dangerous commands ask you to press Return a
second time first; Esc cancels. **Unverified**: Finder trash scripts have only had a read-only contract test.

## Notifications from workflows do not appear

They are posted with `osascript display notification`. Check System Settings > Notifications for the entry macOS lists for
it (usually "Script Editor") and Focus modes. **Unverified** on a clean account.

## Workflows

| Symptom | Cause and fix |
|---|---|
| The keyword does nothing | `ashdash workflows`: is it listed, and not `(disabled)`? (Preferences > Workflows has the checkbox.) `ashdash query "<keyword> test"` shows what the workflow returns without the window. Edits to workflow files are picked up by themselves within about a second; `ashdash reload` forces a re-scan. |
| A row "<workflow name>: script error" | The script failed or printed something that is not Script Filter JSON/XML. The row's subtitle is the first line of the error. Run the script yourself in a terminal with the same arguments. Set `ASHDASH_WORKFLOW_DEBUG=1` to see Debug objects. |
| Slow filter, then an error | A Script Filter has 10 s, an action 60 s, and output is capped at 8 MiB. |
| Works in the reference launcher, not here | Compare the object types with [WORKFLOWS.md](WORKFLOWS.md). Unsupported objects are skipped. Typical causes: a script needs an `alfred_*` variable Ashdash does not set; the workflow relies on a variable set from data that Ashdash protects (`PATH`, `BASH_ENV`, ...); look at `info.plist` for `alfred.workflow.trigger.*` types other than hotkey/external; the script uses `/usr/bin/python3` and the Xcode command line tools are not installed (the first run then offers to install them). |
| `ashdash import` says `refusing to install without confirmation: no terminal to ask on` | It always asks. Run it in a terminal, or pass `--yes` when you have already looked at the package. `not installed` means you answered anything but `y`/`yes`. |
| The import prompt does not install | Launcher window: Return counts only as a fresh press after 600 ms (a held Return is ignored), Esc cancels. Finder dialog: the default button is Cancel and the dialog gives up after 5 minutes. Preferences: use the Install button. |
| `ashdash import` fails | `not a valid workflow archive` (not a zip), `archive has no readable info.plist` (the file must be at the top of the zip), `workflow has neither a bundle id nor a name`, or a limit (2,000 entries, 100 MiB expanded, compression ratio) / an unsafe path inside the archive. |
| Double-clicking a `.alfredworkflow` does not start Ashdash | Needs the app bundle (`Ashdash.app`) in `/Applications`; the bare binary does not register the file types. If the reference launcher is installed macOS may prefer it as the default app: right-click > Open With > Ashdash. |
| A workflow hotkey does nothing | Same rules as the main hotkey; a clash with another workflow or the launcher prints a warning on stderr and the first one wins; a disabled workflow has no hotkeys. Hotkeys are re-registered after an import and when workflows, `workflow_dirs` or the launcher hotkey change; `ashdash reload` forces it. Hotkeys run the workflow by its folder name, so a workflow with no bundle id works too; two workflow folders with the same name in different roots do not (the second is refused with a warning). |
| `ashdash://trigger` is refused | The workflow must be enabled, the trigger must be an External Trigger object, and you must have answered the first-use dialog with Allow. Your answer is stored in `<data>/url-trust.json`; delete the entry to be asked again (a running launcher notices the change). A dialog you ignore for a minute is not stored as "deny"; the next link asks again. More than 6 URLs in 10 s are dropped. |

## The preferences window

* `ashdash prefs` (or the menu-bar item, or `open ashdash://prefs`) opens it as its own program with a Dock icon. If nothing appears,
  run `ashdash prefs-window` from a terminal and read stderr. `ashdash: the preferences window is already open` means one exists:
  it is brought forward (look for a second Space or a minimised window).
* Banners at the top: "Warning: ..." (red) shows `config.json` problems (an unreadable file: the window shows defaults and will
  not change the file) and the last problem the launcher met while applying settings (it clears when the next reload works);
  "Restart Ashdash for these to take effect: app_dirs" (yellow); "Ashdash's launcher is not running: changes apply the next
  time it starts" or "The launcher has not reported yet" (yellow) mean nobody is applying changes. Changes are applied by the
  launcher, not by the window, so start it (`ashdash show`).
* "Forget what I picked..." asks the running launcher to clear `usage.json`; if none runs it clears the file directly.

## URL scheme, AppleScript, login item, file types

All of these need `Ashdash.app` (they are declared in the bundle's `Info.plist`). Running `cargo run` does not register them.
`ashdash login-item status` explains what is missing. Test the URL scheme with `open "ashdash://show"`.
Remember `reference://` belongs to the reference launcher; Ashdash never registers it.

## Settings were not applied

* Settings apply to a running Ashdash within about a second (live reload). Only `app_dirs` needs a restart; the preferences window says so. If nothing happened, look at the Preferences banner and stderr: an unreadable `config.json`, an unusable hotkey or a refused web search is reported there, and the previous setting stays in force.
* The reload is triggered by a change of the file's contents after a 300 ms quiet period; a tool that writes `config.json` in a way the file watcher cannot see (for example a different volume) needs `ashdash quit` and a restart.
* `ashdash: ignoring invalid config.json: ...` on stderr: the file did not parse, so defaults are in use. Run
  `ashdash config get` (it reports the error and does not overwrite your file) and fix the JSON. `ashdash config set` refuses
  to write while the file is unreadable.
* A `version` newer than this build knows: saving is refused so that nothing is lost.
* Keys this build does not know are dropped the next time Ashdash (preferences, `config set`) saves the file.
* `~` and `~/...` work in `app_dirs`, `workflow_dirs`, `theme` and `file_search.*`; `~user/...` does not. Saving keeps the `~` you wrote, `ashdash config get` shows the expanded path.

## macOS refuses to open the app

Ad-hoc signed builds are not trusted by Gatekeeper: right-click > Open, or
`xattr -dr com.apple.quarantine /Applications/Ashdash.app` ([RELEASING.md](RELEASING.md)). Notarization is not done yet.

## Ashdash quit unexpectedly

When the launcher or the preferences window panics, Ashdash writes a report to
`~/Library/Application Support/ashdash/crashes/crash-<UTC time>.txt` (the newest 10 are kept; `ashdash paths` prints the folder and
the newest file). The next time you open the launcher, its footer says "Ashdash quit unexpectedly · report in crashes/" once.

A report holds: Ashdash's version, which process it was (`launcher` or `prefs-window`), the thread, the panic message and its source
location, and a backtrace. It does not contain what you typed in the launcher, clipboard content, file contents or your settings,
and it is never sent anywhere. The panic message is text written by the code that panicked, so read the file before you attach it
to an issue. A panic on a background thread also writes a report even if Ashdash kept running. A crash that is not a panic (the
system killing the process, a crash inside macOS libraries) leaves no report here; look in Console.app > Crash Reports.

## Start over

```sh
ashdash quit
mv ~/Library/Application\ Support/ashdash ~/Desktop/ashdash-backup     # settings, snippets, workflows, history
rm -rf ~/Library/Caches/ashdash                                         # safe: only caches
```

Move things back one at a time to find the one that matters. `usage.json` (what you launched) and `clipboard.json`
(history) can be deleted on their own; so can `workflows/` entries.

## Reporting a bug

Open an issue (templates under `.github/ISSUE_TEMPLATE/`) with: Ashdash version and commit (`git rev-parse --short HEAD`),
macOS version and architecture, whether you run the bundle or `cargo run`, exact steps, expected and actual result, and the
stderr output, and the newest `crashes/` report if Ashdash quit. Do not attach screenshots of your whole desktop, clipboard history or `usage.json`. Security problems: see `docs/SECURITY.md`. No private
reporting channel has been set up yet (open item for the maintainers); until then do not publish exploit details in an issue.

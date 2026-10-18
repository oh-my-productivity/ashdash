# Ashdash: product values

Ashdash is a keyboard launcher for software engineers on macOS. The promise: the shortest path from intent to done, every
time, without thinking about the tool. A launcher is opened hundreds of times a day, so small costs compound: 100 ms of
delay, or one wrong result in a thousand, is paid every day.

This file says what the product optimises for, how each value is measured, where `main` stands (with evidence), and what
wins when values conflict. Every work package names the values it serves and must not regress a bar; reviews check that.
State below is `main` at `1cb6523` (2026-10-17). Status words: **met**, **partly**, **not met**, **not measured**.

Who it is for: engineers who live on the keyboard, script their environment, keep config in dotfiles and handle secrets
(tokens, keys) every day. Not a goal: a mouse-first app or a cloud service.

## 1. Speed

It is never the thing you wait for.

| Bar | Today | Status |
|---|---|---|
| Hotkey to window visible, p50 <= 100 ms | p50 72 ms, p95 83 ms (synthetic hotkey, `docs/PERF.md` "Wave 4 round 3") | met |
| Query p95 < 10 ms at 5,000 apps, scripts excluded | 0.42 ms engine side; the GUI frame is not included | met (engine) |
| Cold start to hotkey-ready < 300 ms | 114-180 ms; 293-323 ms on a first-ever launch with 5,000 apps | partly |
| Idle CPU about 0, idle footprint < 80 MB | 0.01 s per minute; 34-41 MB | met |
| Keystroke to updated list within one frame (built-in providers) | | not measured |

Known cost: about +185 MB footprint for roughly 10 s after each show. That is the first GPU frame, mostly the Metal driver (`docs/PERF.md`).

## 2. Predictability

The same keystrokes do the same thing, so muscle memory is safe.

| Bar | Today | Status |
|---|---|---|
| The same query on the same data gives the same order (deterministic tie-break) | Engine merge sorts by score, title (case-folded), exact title, id; a NaN score sorts last and `-0.0` is `0.0`. `providers::order_tests` shuffles equal-score candidates through all 5,040 input orders and through two folder orders. Providers that cut their own list before the merge (bookmarks general limit, exclusive workflow lists) are not checked | partly |
| Once you have moved the selection, results that arrive later never change which item is highlighted | The final answer keeps the item you moved to by id (also after "back" from a panel); if you did not move, the new top is selected; if the item is gone, the top. A `rerun` refresh never makes Return or Cmd+number refuse (it acts on the row you saw if its id is still there, else on the highlighted row) (`SelectionTrack` in `gui/schedule.rs`, used by `gui/workers.rs`; tests `final_answer_*`, `the_pending_rows_*`). Logic is unit-tested. Seen on screen in the lab (QA run 2026-10-rc1, W5.1/W5.2a/W5.2b): a Script Filter whose rerun reorders the list kept the highlighted row by id in 3 of 3 trials, and the slow answer left the moved-to row selected. The panel-back and rerun-Return cases are unit-tested only | met |
| Return runs the item highlighted in the frame you saw | Return and Cmd+1..9 resolve against the list drawn before the frame in which a result list replaced it; a row that is gone runs nothing and the footer says so (tests `return_*`, `cmd_number_*`, `several_replacements_*`). A mouse click is not covered. Seen on screen (W5.2b): after a slow Script Filter answered, Return ran the row that was on screen. The "same frame" case (W5.2c) cannot be provoked from a script and stays unit-test only. | partly |
| Learned order never ranks a result above an exact keyword or exact name match; learning can be turned off | `learn_usage` opt-out and "Forget" exist; the ranking rule is not checked | partly |

## 3. Reliability

It is there every time you press the hotkey.

| Bar | Today | Status |
|---|---|---|
| Hotkey and window survive sleep/wake, display connect/disconnect, Spaces, full-screen apps and an app update | on the QA list, never run QA run 2026-10-rc1 (VM): shows over a full-screen app (3.1.3a), hotkey works after a display sleep/wake of the virtual display (SOAK-PM), after 2 h of use (SOAK), and after a hotkey change (3.1.6a). Not possible in the VM, steps for the owner in `docs/qa-runs/2026-10-rc1.md`: second display / Spaces / hot-plug / real sleep (the guest cannot sleep) / app update. | partly |
| 7-day soak with use every few minutes: no crash, footprint drift < 10 % | 60 s idle runs and 20 show/hide cycles only 2 h soak in a lab VM (240 show/type/hide cycles, one every 30 s): 0 missed hotkeys, 0 crashes, RSS +2.1 % (91.2 -> 93.2 MB), footprint 31 -> 33 MB, threads and open files constant (`docs/qa-runs/2026-10-rc1.md`). The 7-day run with real use is still to do. | partly |
| No outside input (packages, themes, plists, URLs, clipboard, script output) can abort the process | Every class found so far is fixed with tests: plist depth and value count, calculator recursion, multibyte URLs (`docs/SECURITY.md`, `crates/core/tests/fuzz.rs`). This bar is never "done" | partly |
| After a crash you can find out why | Panic hook in the launcher and the preferences window writes `<data>/crashes/crash-<UTC>.txt` (version, process, pid, thread, outcome fatal/recovered, message cut to its first line and 200 characters, location, backtrace; last 10 kept; an identical repeat is counted in its report), a one-time footer note on the next launch ("quit unexpectedly" for fatal, "hit an internal error and carried on" for recovered panics), and `ashdash paths` names it (`crates/app/src/crash.rs`, `core/src/recovery.rs`, 15 tests). The message may quote text the program was working on; the report and TROUBLESHOOTING say so. Crashes that are not panics (killed process, native crash) leave nothing there; no real panic of the running app was provoked. Seen on screen in the lab (W5.3a/b, before the fatal/recovered wording): a report newer than `last-seen` gives the footer note once, not on the second show, and `ashdash paths` names it | partly |

## 4. Flow

You never leave the keyboard, and the tool never interrupts you.

| Bar | Today | Status |
|---|---|---|
| Every action reachable by keyboard | Launcher: yes (modifiers, Universal Actions panel, file buffer, Esc). Preferences window: not audited | partly |
| On hide, focus returns to the app you came from | smoke test (`docs/PERF.md` notes). Lab run (3.1.4): after Esc the previous app (TextEdit) is frontmost again and receives the next keystroke. Separate bug, not this bar: the launcher does not always hide when another app is activated (3.4.1, `docs/qa-runs/2026-10-rc1.md`). | met |
| No UI you did not ask for, except trust/consent prompts for foreign code and results of your own actions | by design: no update nags, no notifications of its own | met |

## 5. Trust (security and privacy)

Your secrets stay yours, and foreign code is described truthfully before it runs.

| Bar | Today | Status |
|---|---|---|
| Nothing leaves the machine except through an action you take | no telemetry, no update check, no HTTP client among the crates' dependencies | met |
| The import prompt never calls a workflow passive when it can run code, launch apps, write files or open URLs | four review rounds (`docs/reviews/`, `docs/SECURITY.md`); the round 3 fixes are gate-tested, not re-reviewed | met |
| Concealed and transient clipboard entries, and Ashdash's own snippet pastes, never reach history | covered by tests; the real pasteboard is a QA item | partly |

## 6. Programmability

You extend it in the language you already use, and drive it from anything.

| Bar | Today | Status |
|---|---|---|
| The reference launcher workflows run unchanged | every bundled reference launcher template imports and loads (`crates/core/tests/compat_packages.rs`); object coverage in `docs/WORKFLOWS.md` | partly |
| Everything the launcher does can be done from the CLI, socket, `ashdash://` or AppleScript | `ashdash query`, `docs/AUTOMATION.md`; no audit that every action has an equivalent | partly |
| A new workflow takes minutes to write | no scaffold, no schema docs, no editor (the editor is not on the first release) | not met |

## 7. Debuggability

When something misbehaves, you see why in seconds.

| Bar | Today | Status |
|---|---|---|
| For the last runs of any workflow, see input, variables, stdout/stderr, exit code and duration, in the app and from the CLI | `ASHDASH_WORKFLOW_DEBUG` prints to stderr; `ashdash query` runs a search headless; `run/reload-status.json` reports reload problems | not met |
| Every error you see names the file or workflow and the next step | | not audited |

## 8. Config as code

Your setup lives in your dotfiles and comes back on a new machine in one command.

| Bar | Today | Status |
|---|---|---|
| Settings are text files that diff cleanly; unknown keys survive a save | `config.json`, `ashdash config set`; unknown keys kept since W4-FIX; stable key order not checked | partly |
| Paths are portable: `~` works wherever a path is accepted | expanded in `file_search.*`, `workflow_dirs`, `app_dirs` and `theme` when loaded; a save keeps the `~` the file has (`config::tests::tilde_*`, `a_save_keeps_the_tilde_the_user_wrote`, `engine::tests::tilde_paths_in_the_settings_file_are_used_as_home_paths`). `~user` is not expanded | met |
| One command exports, and one applies, the whole setup (config, workflow list, snippets, themes) | | not met |

## 9. Developer context

It knows your world: projects, repositories, terminals, editors, hosts.

| Bar | Today | Status |
|---|---|---|
| With zero setup, each in at most 4 keystrokes after the hotkey and within the speed bars: open a recent project in its editor; jump to a git repository or worktree; ssh to a host from `~/.ssh/config`; see what listens on a port and stop it | built-ins are generic (apps, files, path browse, `>` Terminal, bookmarks, snippets, clipboard history); developer features exist only through third-party workflows | not met |

This value decides whether Ashdash is more than a free reference launcher clone.

## 10. Ownership

You own the tool and your data.

| Bar | Today | Status |
|---|---|---|
| Open source, no account, no subscription, works offline | MIT OR Apache-2.0, clean-room (`docs/CLEAN_ROOM.md`) | met |
| Builds from source with one command | `cargo build --release`; `packaging/build-app.sh` for the app bundle | met |
| Signed and notarized, so installing needs no workaround | ad-hoc signed until a Developer ID is available | not met |
| The name is free to use | Renamed to Ashdash on 2026-10-18; a quick web search found no launcher of that name, which is not a legal clearance (`docs/CLEAN_ROOM.md`) | open |

## When values conflict

1. **What runs must be right before it is fast.** Never show or run a stale or wrong result to save milliseconds (predictability over speed).
2. **Predictability over cleverness.** Learning, ranking tweaks and suggestions must not break the bars in section 2. When in doubt, ship the feature off by default.
3. **Trust at the boundary, no friction inside.** Prompt only where foreign code or data enters (import, `ashdash://`, workflow folders). Never prompt for what you typed or set up yourself.
4. **Programmability inside trust.** Workflows may do anything the reference launcher's can, but the prompt says so truthfully.
5. **The reference launcher compatibility over stricter defaults, only where the risk is visible.** For example, SEC-03/13/14 stay open by design and the trust prompt reports them.
6. **Speed and calm over features.** A built-in that cannot meet the speed bars, or costs idle CPU, ships as a workflow or not at all.

## What this changes in the plan

Add to the v0.1.0 release candidate (small):
- Predictability: keep the selection by result id when the final answer replaces pending results (`workers.rs:147-150`), with a test. **Done on `wp/w5-fix`.**
- Reliability:
  - a panic hook that writes a crash report to the data folder and names it on the next launch (**done on `wp/w5-fix`**);
  - sleep/wake, display change, full-screen and a 24 h soak in the VM QA run (`docs/QA.md`).
- Config as code: expand `~` in `workflow_dirs`, `app_dirs` and `theme` (**done on `wp/w5-fix`**).

v0.2 headline:
- Developer pack (section 9) as built-in providers under the speed bars.
- Workflow run log and debugger view, plus a CLI equivalent (section 7).
- `ashdash export` / `ashdash apply` (section 8) and a `ashdash new workflow` scaffold (section 6).
- Keyboard-navigable preferences; start the event loop before the first cold scan (section 1).

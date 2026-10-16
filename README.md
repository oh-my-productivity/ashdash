# Ashdash

A keyboard launcher for macOS written in Rust: press a hotkey, type, press Return. Ashdash is modelled on the observable
behaviour of the reference launcher and runs many reference-compatible workflows, but it is an **independent project: not affiliated with,
endorsed by or sponsored by its publisher or the reference launcher application, and it contains no reference launcher code or assets**
([docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md)). "Ashdash" is a working name and will probably change before a public release.

Status: pre-release (0.1.0), no published binaries yet. Read [docs/USER_GUIDE.md](docs/USER_GUIDE.md) for what is verified and
what is not.

```
cargo run -p ashdash --release        # Option+Shift+Space opens it; Esc closes; Cmd+Q quits
cargo run -p ashdash -- query "vsc"   # headless: print ranked results
```

Features: app launcher with initials/word-prefix matching and learned ranking, calculator (`2+3*4`, `= sqrt(81)`),
keyword web searches (`g rust`, `wiki x`), file search (`open name`, `find name`, `in text` via Spotlight, `kind:pdf`),
path browsing (`~/Doc`), system commands (destructive ones ask to confirm), browser bookmarks (`bm`), clipboard history
with images and file lists (`clip`), snippets with placeholders (`snip`), Universal Actions for text and files (the `actions` keyword, and
Right Arrow on a result, with a file buffer on Option+Up/Down), run in Terminal (`> cmd`), `.alfredappearance` themes, a
**preferences window** (`ashdash prefs`) with live reload of settings, and **reference-compatible workflows**
(`ashdash import x.alfredworkflow`, which always shows what the workflow can do and asks first):
keyword inputs, Script Filters (JSON and XML), list and file filters, scripts, conditionals and other utilities, Open
URL/File, notifications, clipboard, Large Type, external triggers, workflow hotkeys.
Also: menu-bar item, `ashdash` command line, `ashdash://` URLs, AppleScript, launch at login.

Not yet: contacts, dictionary, system-wide snippet expansion, a workflow editor.

## Documentation

| | |
|---|---|
| [docs/USER_GUIDE.md](docs/USER_GUIDE.md) | install, first run, permissions, hotkeys, every keyword |
| [docs/SETTINGS.md](docs/SETTINGS.md) | every `config.json` key with type, default, example |
| [docs/WORKFLOWS.md](docs/WORKFLOWS.md) | workflow compatibility per object type, trust model |
| [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) | symptoms and fixes |
| [docs/AUTOMATION.md](docs/AUTOMATION.md) | CLI socket, `ashdash://`, AppleScript, workflow hotkeys |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | crates, providers, engine flow, threads, data folders |
| [CONTRIBUTING.md](CONTRIBUTING.md) | build, test, branches, how to add a provider |
| [docs/SECURITY.md](docs/SECURITY.md) | threat model and findings |
| [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md) | provenance, trademark and naming notes |
| [docs/RELEASING.md](docs/RELEASING.md), [docs/QA.md](docs/QA.md), [docs/PLAN.md](docs/PLAN.md), [docs/STATUS.md](docs/STATUS.md) | release, test and project status |

## Configuration in one minute

Settings are one JSON file, `~/Library/Application Support/ashdash/config.json` (`ashdash paths` prints every location).
Change a setting in the preferences window (`ashdash prefs`) or from the shell; a running Ashdash applies it within about a
second (only `app_dirs` needs a restart):

```sh
ashdash prefs                                # preferences window
ashdash config keys                          # list every setting
ashdash config set clipboard.max_age_days 14
ashdash config set providers_disabled calc,system
```

Per-workflow settings live in `workflow-config/<bundle id>.json` next to it. Full reference: [docs/SETTINGS.md](docs/SETTINGS.md).

## Security and privacy

Workflows run arbitrary scripts with your permissions, so only import ones you trust ([docs/WORKFLOWS.md](docs/WORKFLOWS.md)).
Every import (command line, Finder, launcher window, preferences) first shows what the workflow can do (scripts, Terminal commands,
file writes, clipboard, URL schemes, external files) and installs only after you confirm; workflow folders you add yourself
through `workflow_dirs` in `config.json` are not asked about.
Everything stays on your Mac. The clipboard history is stored as plain JSON (mode 0600) under the data folder; entries that
password managers mark as concealed, transient or auto-generated, and copies from apps on an ignore list, are skipped
(the concealed filter is not yet verified with a real password manager). Usage learning stores the queries you ran in `usage.json`; it can be switched off in Preferences (`learn_usage`), which does not
delete what was stored: "Forget what I picked..." does.

## Layout

`crates/core` (engine, providers, workflows, stores; no UI, unit-tested), `crates/app` (egui window, hotkeys, CLI, control
socket, URL scheme), `packaging/` (app bundle, DMG, notarization, cask), `docs/`, `scripts/`.

## License

Licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>), or
* MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option. SPDX-License-Identifier: `MIT OR Apache-2.0`. Third-party licences are listed in [NOTICE](NOTICE).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined
in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

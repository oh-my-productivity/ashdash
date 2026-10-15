# ashdash

A keyboard launcher for macOS in Rust, modelled on the reference launcher's observable behaviour
(see [docs/REVERSE_ENGINEERING.md](docs/REVERSE_ENGINEERING.md)).

```
cargo run -p ashdash --release        # Option+Space opens it; Esc closes; Cmd+Q quits
cargo run -p ashdash -- query "vsc"   # headless: print ranked results
```

Features: app launcher with initials/word-prefix matching and learned ranking,
calculator (`2+3*4`, `= sqrt(81)`), keyword web searches (`g rust`, `wiki x`),
file search (`open name`, `find name` via Spotlight), path browsing (`~/Doc`),
system commands (destructive ones ask to confirm), clipboard history (`clip`),
snippets (`snip`, `snippets.json`), run in Terminal (`> cmd`), `.alfredappearance`
themes, and **reference-compatible workflows** (`ashdash import x.alfredworkflow`):
keyword inputs, Script Filters (JSON), Run Script, Open URL/File, notifications,
clipboard, Large Type, Arg & Vars, modifier overrides, external triggers.

Config: `~/Library/Application Support/ashdash/config.json` (hotkey, `theme`,
`workflow_dirs`, `web_searches`). Run `ashdash paths` to see locations.

Security notes: workflows run arbitrary scripts, so only import ones you trust.
Clipboard history is stored in plain JSON (mode 0600) and cannot skip entries
that password managers mark as concealed.

Crates: `crates/core` (engine, no UI, unit-tested), `crates/app` (egui window, hotkey, CLI).

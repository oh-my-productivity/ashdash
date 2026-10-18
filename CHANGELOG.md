# Changelog

All notable changes to Ashdash are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/) (before 1.0, minor versions may change behaviour).
A claim that nobody has been able to check on a real desktop or machine is marked **unverified**; the full list is in
[docs/USER_GUIDE.md](docs/USER_GUIDE.md#10-known-unverified-and-missing).

Releases add their section automatically from the commit messages (see [docs/RELEASING.md](docs/RELEASING.md#changelogmd-and-the-release-notes)); do not edit those sections.
Hand-written text under `## [Unreleased]` is folded into the next release.

## [0.1.0] - 2026-10-18

First public version. Verified by `scripts/check.sh` (format, clippy with warnings as errors, all tests);
on-screen behaviour was verified only where noted.

### Added

- Launcher window opened with a global hotkey (default Option+Shift+Space), menu-bar item, launch at login (`ashdash login-item`), `ashdash` command line, `ashdash://` URLs, AppleScript dictionary.
- Apps (persistent index kept fresh with FSEvents, fuzzy/initials matching, learned ranking that can be switched off and forgotten), calculator, keyword web searches, system commands (destructive ones ask first),
  path browsing, `>` run in Terminal, file search through Spotlight (`open`, `find`, `in`, `kind:`), browser bookmarks (`bm`).
- Clipboard history (text, images, file lists; concealed/transient entries and an ignore list of apps are skipped), snippets with placeholders, auto-paste (needs Accessibility; **unverified** with a real Cmd+V and a real password manager).
- Universal Actions panel (Right Arrow) and file buffer, Quick Look, Large Type.
- Workflows (formats: [docs/WORKFLOWS.md](docs/WORKFLOWS.md)): keyword and Script Filter inputs (JSON and XML, `cache`, `rerun`, `runningsubtext`, user configuration), list and file filters, scripts, conditionals and other utilities, Open URL/File,
  Write File, Terminal, notifications, clipboard, external triggers, workflow hotkeys. Importing always shows what the workflow can do (scripts, Terminal commands, file writes, clipboard, URL schemes, external files)
  and installs exactly the previewed copy only after confirmation. Compatibility per object type: [docs/WORKFLOWS.md](docs/WORKFLOWS.md).
- Themes (appearance files); preferences window (`ashdash prefs`: General, Features, Workflows, About) with live reload of `config.json`, workflows and snippets.
- Packaging: universal (arm64 + x86_64) ad-hoc signed `Ashdash.app`, DMG with checksum, notarization and Homebrew cask scripts that need credentials the project does not have yet.
- Security review findings SEC-01..20 and SEC-30..34 each have a recorded status in [docs/SECURITY.md](docs/SECURITY.md) (SEC-03, 13 and 14 stay open by design, to stay compatible with existing workflow packages).
- Licence `MIT OR Apache-2.0`, issue templates, `cargo deny` licence and ban checks.

### Fixed

- Chinese, Japanese and Korean text in the launcher window is drawn with a system font (loaded the first time such text is shown) instead of empty boxes.
- `ashdash login-item status` says "off" on a fresh install instead of a misleading "not found" warning.

### Changed

- Themes: a theme offered for installation (Finder, `ashdash theme <file>`, the preferences window) is limited to 2 MiB, but a theme that is already installed or selected in the settings
  loads up to 8 MiB.
- Workflows: Copy to Clipboard now honours the "transient" box (the text is marked so clipboard history does not record it). Without it, copies stay recorded like your own.

### Release engineering

- `docs/PERF.md`: measured query, start-up, clipboard-search and file-search-budget numbers (`crates/core/benches/report.rs` now reports 500 and 5,000 apps, clipboard history search and the file-walk budget).
- Releases are one click and automatic: the version number is computed from the Conventional Commits since the previous release (`scripts/release/next-version.sh`, `cliff.toml`; in 0.x a breaking change or `feat` bumps the minor version, `fix` or `perf` the patch version, other types alone release nothing), the `CHANGELOG.md` section and the release notes are generated, the release commit and the annotated tag are pushed to the protected `main` with a deploy key, and the GitHub release is created as a **draft** by a separate job. Details, one-time setup and recovery: [docs/RELEASING.md](docs/RELEASING.md). Third-party actions are pinned by commit SHA; git-cliff is pinned by version and SHA-256.
- Advisories (RustSec) run in a weekly `advisories.yml` workflow; the pull-request `deny` job checks `licenses bans sources` only.
- Fixed: `packaging/selftest.sh` failed whenever `shellcheck` was installed; `dist/` is ignored by git.

### Not included

Contacts and dictionary lookup, System Settings panes, history search, system-wide snippet expansion (needs an event tap), music control, a workflow editor, the trust-prompt extras (diff on replacement, imports disabled until enabled).

### Known issues

See the [v0.1.0 release notes](docs/release-notes/v0.1.0.md#known-issues).

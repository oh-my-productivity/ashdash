# Contributing to Ashdash

Thank you for helping. The repository is <https://github.com/oh-my-productivity/ashdash>. Ashdash is a macOS keyboard launcher written in Rust; the
project is independent of the reference launcher application ([docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md)). Read
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) first: it explains the two crates, the provider model and the threads.

## Ground rules

* **License.** The project is `MIT OR Apache-2.0` ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
  Unless you state otherwise, any contribution you submit for inclusion is licensed under both, without additional terms or
  conditions (the usual inbound = outbound rule of the Rust ecosystem; Apache-2.0 section 5 says the same). Do not submit code
  you are not allowed to license this way.
* **No reference launcher code or assets.** Never copy source, icons, themes, sample workflows, text or screenshots from the reference launcher or its
  documentation into this repository. Learn behaviour from public documentation and from observing file formats, and write
  your own code. Tests that read an installed reference launcher must skip cleanly when it is absent and must never commit what they read.
* **Never use real user data in tests or examples.** No real clipboard contents, bookmarks, history, snippets or the contents
  of `~/Library/Application Support/reference-launcher`. Use synthetic data in a temporary directory.
* **No destructive actions for real.** Tests and scripts must never shut down, log out, empty the trash or delete user files.
* **Be honest in docs.** If you could not verify something on a real Mac, say "unverified" where you describe it; the running list
  is "Known unverified items" in [docs/STATUS.md](docs/STATUS.md).

## Setup

Requirements: macOS (the app uses AppKit), the Xcode command line tools, and Rust 1.88 or newer (let-chains are used; the
maintainers build with 1.95; CI uses the current stable toolchain). Edition 2024.

```sh
git clone <repository-url> ashdash && cd ashdash
cargo build -p ashdash                  # debug binary at target/debug/ashdash
scripts/check.sh                        # the gate: fmt, clippy -D warnings, all tests
```

Run commands that do not open a window with a throw-away profile so your own settings and history stay untouched:

```sh
HOME=$(mktemp -d) target/debug/ashdash --help
HOME=$(mktemp -d) target/debug/ashdash config keys
HOME=$(mktemp -d) target/debug/ashdash query "saf"
```

`ashdash` with no arguments starts the real launcher: it registers a **global hotkey**, a control socket and a status item,
and it reads your clipboard. Do that with a temporary `HOME`, never while another instance is running, and not in an environment
shared with other people or agents. See [docs/QA.md](docs/QA.md) for the manual checklist and `scripts/qa/gui-smoke.sh`.

## Workflow: branches, worktrees, review

* One topic per branch: `wp/<name>` for a work package, `fix/<name>` for a bug fix. Branch from `main`.
* The maintainers use one git worktree per branch under `.worktrees/` (ignored by git), so several branches can be built at once
  without stepping on each other:

  ```sh
  git worktree add .worktrees/<name> -b wp/<name> main
  cd .worktrees/<name>
  ```

  Plain clones and forks work equally well; open a pull request from your branch.
* Commit small steps with a message that says why. Do not `git push --force` to shared branches and do not rewrite history that
  others have fetched.
* **Gate for every change:** `scripts/check.sh` must print `check.sh: OK`. CI (`.github/workflows/ci.yml`) runs it on macOS,
  and a second job runs `cargo deny` ([below](#dependencies-and-licences)).
* Risky areas (workflow execution, import, anything that runs processes or touches the clipboard) get a behavioural code review
  before merging; say in the pull request what could go wrong.
* Merge hot spots where concurrent branches collide: `crates/core/src/engine.rs` (its test module in particular),
  `crates/core/src/providers/mod.rs`, `crates/app/src/main.rs`, `crates/app/src/gui/mod.rs`, `Cargo.toml`, `Cargo.lock`. Keep
  edits there small and local, and do not reformat. Never run `cargo generate-lockfile`; add dependencies with an exact version
  and say why.
* Fix a bug with a regression test in the same commit. A test marked `#[ignore = "known: SEC-nn ..."]` documents an open
  finding in [docs/SECURITY.md](docs/SECURITY.md); un-ignore it in the commit that fixes it and update the status column there.

## Tests

* Unit tests sit next to the code (`#[cfg(test)] mod tests`); integration suites are in `crates/core/tests/`
  (`engine_e2e`, `import_security`, `scripts_security`, `fuzz` with proptest, `compat_packages`). Commands and run times:
  [docs/QA.md](docs/QA.md).
* Use a temporary directory and `Paths::under(&root)`; use `Engine::with_env` with a fake `Os` (inside the core crate: `actions::testing::FakeOs`) so
  nothing touches the real machine. Tests of the core crate that use `providers::ambient_home()` get a folder that does not exist.
* The launcher's pure logic (key names, monitor placement, scheduling) is kept in functions that take plain values so they are
  unit-testable without a window. Window behaviour is verified by hand or with the QA script and described in the pull request.
* Anything that shows a window, registers a hotkey, URL handler or login item, or sends keystrokes must not run on a shared
  desktop. A VM-based test lab (`docs/PLAN.md` section 8) is planned for that.

## How to add a provider

A provider answers "what rows do you offer for this query?" and lives in `crates/core/src/providers/`.

1. **Create `providers/<name>.rs`** with a type that implements `Provider`:

   ```rust
   use super::{Provider, ProviderOutput, QueryCtx, strip_keyword};
   use crate::item::{Action, Icon, Item};

   pub struct TimeProvider;

   impl Provider for TimeProvider {
       fn id(&self) -> &'static str { "time" }               // stable; users put it in `providers_disabled`
       fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
           let Some(_rest) = strip_keyword(ctx.query, "time") else { return ProviderOutput::none() };
           let now = "12:00";                                 // compute the real value here
           let item = Item::new("time:now", now)
               .subtitle("Return copies it")
               .icon(Icon::Glyph("⏱"))
               .action(Action::Copy(now.into()));
           ProviderOutput::exclusive(vec![item])              // or ::ranked(items) to be merged and ranked
       }
   }
   ```

   Return `ProviderOutput::exclusive(..)` only when your keyword takes over the whole list (then later providers are not asked);
   otherwise `ranked(..)` and give each `Item` a `score`. Override `learns()` to return `false` when your provider orders its own
   results, and `placement()` for `Trailing` rows that must close the list. Read configuration and shared stores through
   `ctx.engine_state`. Do OS side effects (`open`, `mdfind`, `osascript`) through the `Os` trait or `Action` values, never
   inline in `query`, and keep `query` fast: it runs on every keystroke, on the single query worker thread. Do not block on
   slow I/O there; read from a cache that a background thread keeps fresh (see `bookmarks`).
2. **Register it in `providers/mod.rs`**, in three places that must stay in step: `mod <name>;`, the id in the `PROVIDER_IDS`
   list, and one `add!("<id>", <name>::Type)` line in `registry_for` at the position where it should take precedence. The id
   literal must equal `Provider::id()`; the tests `provider_id_list_matches_the_registry`, `registry_ids_are_unique` and
   `disabled_providers_are_not_created` check this. (The module comment at the top of `mod.rs` still says "ONE line"; it is out of date.)
3. **Test it** inside the provider file with the shared fixture `providers::testutil::engine(tag)`, which builds an `Engine` over a
   temporary directory: assert on `engine.query("...")` rows, on exclusivity and on `Engine::execute` outcomes with a fake `Os`.
4. **Document it**: the keyword in [docs/USER_GUIDE.md](docs/USER_GUIDE.md) (section "What to type") and the id in
   [docs/SETTINGS.md](docs/SETTINGS.md) (provider ids). A new setting also needs a `Config` field with a default, a migration
   note if the schema changes (`config::CURRENT_VERSION`), and a row in SETTINGS.md.

Features that are not a query provider have other entry points: a new window feature goes in `crates/app/src/gui/`, a new CLI command in
`main.rs` (add it to the `HELP` text), a new automation verb in `ipc.rs` / `urlscheme.rs` (security-sensitive: read
[docs/AUTOMATION.md](docs/AUTOMATION.md) and [docs/SECURITY.md](docs/SECURITY.md) first), a new workflow object type in
`crates/core/src/workflow/` (`exec.rs` plus the `FLOW_KINDS` list in `compat.rs`, a test with a synthetic workflow, and a row in
[docs/WORKFLOWS.md](docs/WORKFLOWS.md)).

## Dependencies and licences

* Add a dependency only when it is needed; pin it exactly (`=1.2.3`) in the crate that uses it and commit `Cargo.lock`. Prefer
  well-known, maintained crates, check the licence, and explain why in the pull request.
* `deny.toml` is the licence and advisory policy; CI runs `cargo deny` through `EmbarkStudios/cargo-deny-action`. Run it locally
  with `cargo install cargo-deny --locked` and `cargo deny check`. Allowed: permissive licences (MIT, Apache-2.0, BSD, ISC, Zlib,
  0BSD, Unlicense, CC0, Unicode-3.0, ...). Everything else needs an explicit, commented exception in `deny.toml`; copyleft
  (GPL/LGPL/AGPL) will not be accepted. Today's exceptions: `option-ext` (MPL-2.0, used unmodified) and `epaint_default_fonts`
  (OFL-1.1, Ubuntu Font Licence 1.0).
* After changing dependencies run `scripts/gen-notice.py` to regenerate [NOTICE](NOTICE) and commit it
  (`scripts/gen-notice.py --check` tells you whether it is current).
* Dependency licence review result at the time of writing: no problem found. The dependency tree is overwhelmingly
  MIT / Apache-2.0; the licences that need attention are listed above.

## Documentation

User-facing behaviour is documented in `docs/`; if you change a command, key binding, setting or keyword, change the page in
the same commit. Every command and config key shown in the docs must exist: check with `ashdash --help` and `ashdash config keys`.
Do not add screenshots that show anyone's desktop, and no reference launcher artwork.

## Reporting problems

Use the issue templates in `.github/ISSUE_TEMPLATE/`. For security problems read the note in
[docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md#reporting-a-bug): a private reporting channel has not been set up yet.

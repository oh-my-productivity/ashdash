# Contributing to Ashdash

Thank you for helping. The repository is <https://github.com/oh-my-productivity/ashdash>. Ashdash is a macOS keyboard launcher written in Rust.
Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) first: it explains the two crates, the provider model and the threads.
Everyone taking part is expected to follow the [code of conduct](CODE_OF_CONDUCT.md); security problems go through the
[security policy](.github/SECURITY.md), not a public issue.

## Ground rules

* **License.** The project is `MIT OR Apache-2.0` ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
  Unless you state otherwise, any contribution you submit for inclusion is licensed under both, without additional terms or
  conditions (the usual inbound = outbound rule of the Rust ecosystem; Apache-2.0 section 5 says the same). Do not submit code
  you are not allowed to license this way.
* **Original work only.** Ashdash is an original implementation: it has no third-party code or assets beyond the open-source
  dependencies listed in [NOTICE](NOTICE). Do not copy source, icons, themes, sample workflows, text or screenshots from
  other products into the repository; write your own code and artwork. A test that reads files from outside the repository
  must skip cleanly when they are absent and must never commit what it reads. The compatibility test reads workflow templates
  from the folder named by `ASHDASH_COMPAT_TEMPLATES` and is skipped when the variable is unset.
* **Never use real user data in tests or examples.** No real clipboard contents, bookmarks, history, snippets or the contents
  of another application's data folder. Use synthetic data in a temporary directory.
* **No destructive actions for real.** Tests and scripts must never shut down, log out, empty the trash or delete user files.
* **Be honest in docs.** If you could not verify something on a real Mac, say "unverified" where you describe it; the running list
  is section 10 of [docs/USER_GUIDE.md](docs/USER_GUIDE.md).
* **Names.** Workflow and appearance file formats use fixed identifiers (file extensions, object types, script variables). They
  are defined once in `crates/core/src/format_ids.rs` and listed in [docs/WORKFLOWS.md](docs/WORKFLOWS.md); a test
  (`crates/core/tests/names.rs`) fails when they appear anywhere else.

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

* One topic per branch: `feature/<name>` for a feature, `fix/<name>` for a bug fix. Branch from `main`.
* A git worktree per branch under `.worktrees/` (ignored by git) lets you build several branches at once without
  stepping on each other:

  ```sh
  git worktree add .worktrees/<name> -b feature/<name> main
  cd .worktrees/<name>
  ```

  Plain clones and forks work equally well; open a pull request from your branch.
* Commit small steps with a message that says why, written as a [Conventional Commit](#commit-messages-and-pull-requests)
  (`type(scope): subject`). Do not `git push --force` to shared branches and do not rewrite history that
  others have fetched.
* **Gate for every change:** `scripts/check.sh` must print `check.sh: OK`. CI (`.github/workflows/ci.yml`) runs it on macOS
  (job `check`), `cargo deny` runs in the job `deny` ([below](#dependencies-and-licences)), and two more jobs test the lab scripts and
  the packaging inputs and lint every shell script and workflow. Pushes to `main` also run the GUI smoke test on macOS 15 and 26, the
  tests on Intel and a universal build; a nightly workflow runs the scripted QA checklist and a soak. What runs where, and how to
  reproduce a CI failure locally, is in [docs/CI.md](docs/CI.md). Workflows pin every action by commit SHA; use
  `tests/e2e/ci/lint.sh` (shellcheck and actionlint) after changing a script or a workflow.
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
  (`engine_e2e`, `import_security`, `scripts_security`, `fuzz` with proptest, the workflow compatibility suite). Commands and run times:
  [docs/QA.md](docs/QA.md).
* Use a temporary directory and `Paths::under(&root)`; use `Engine::with_env` with a fake `Os` (inside the core crate: `actions::testing::FakeOs`) so
  nothing touches the real machine. Tests of the core crate that use `providers::ambient_home()` get a folder that does not exist.
* The launcher's pure logic (key names, monitor placement, scheduling) is kept in functions that take plain values so they are
  unit-testable without a window. Window behaviour is verified by hand or with the QA script and described in the pull request.
* Anything that shows a window, registers a hotkey, URL handler or login item, or sends keystrokes must not run on a shared
  desktop. Use the VM-based test lab for that ([docs/LAB.md](docs/LAB.md)).

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
Do not add screenshots that show anyone's desktop, and no artwork you did not make.

## Commit messages and pull requests

Ashdash uses [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/). The release version and the changelog are
computed from the messages on `main`, so the format is checked, locally by git hooks and again in CI.

```
type(scope)!: subject

optional body, separated by an empty line

optional footers, for example
BREAKING CHANGE: what breaks and how to migrate
Refs: #123
```

* **Types:** `feat` (new feature), `fix` (bug fix), `perf`, `refactor`, `docs`, `test`, `build` (build, packaging, dependencies),
  `ci`, `chore`, `style` (formatting only), `revert`.
* **Scope** is optional and free: `gui`, `core`, `prefs`, `lab`, `packaging`, `deps`, `gui/prefs` ...
* **Subject:** imperative ("add", not "added"), starts in lower case, no trailing period; the whole header is at most **72 characters**.
  Say what the change does; the body says why.
* **Breaking change:** a `!` before the colon (`refactor(core)!: rename the snippets store`) or a `BREAKING CHANGE: <text>` footer.
* Valid: `feat(gui): add a dark appearance to the preferences window`, `fix: keep the window on the screen that holds the cursor`,
  `docs: explain the import limits`, `ci(deps): bump actions/checkout from 7.0.0 to 7.0.1`.
  Not valid: `Fixed the crash` (no type), `Fix: crash` (type in capitals), `feat: Add x.` (capital, trailing period),
  `feat:add x` (no space after the colon), a 90-character header.
* What the version does with them (while the major version is 0): a breaking change or `feat` makes a minor release, `fix` and `perf` a
  patch release; the other types alone make no release. `1.0.0` is a deliberate manual decision. ([docs/RELEASING.md](docs/RELEASING.md))
* Git-made messages are fine in your local branch: `Merge ...`, `Revert "..."`, `fixup! ...`, `squash! ...`. They never reach `main`:
  pull requests are squash-merged.

**Pull requests are squash-merged, so the pull request title becomes the commit on `main`.** The title must follow the format above;
the commits inside the pull request should too (CI checks both), but only the title is kept. Put the details in the description, not in
the title, and use the pull request template's checklist.

### Install the git hooks

```sh
scripts/setup-hooks.sh              # once per clone: git config --local core.hooksPath .githooks (nothing global is touched)
scripts/setup-hooks.sh --status     # what is installed, which optional tools were found
scripts/setup-hooks.sh --uninstall  # remove it again
git config commit.template .gitmessage   # optional: a commit message template with the cheat sheet (not set for you)
```

The hooks need only git, bash (the macOS system bash is fine) and the Rust toolchain. They use `shellcheck`, `actionlint` and `gitleaks`
when they are installed (`brew install shellcheck actionlint gitleaks`) and say which checks they skipped when not.

| Hook | What it checks |
|---|---|
| `commit-msg` | the message is a valid Conventional Commit (`scripts/commit-lint.sh`) and contains no retired product name |
| `pre-commit` (staged changes only, about a second) | `rustfmt` on staged `.rs` files (the staged content); `shellcheck` on staged shell scripts and `actionlint` on staged workflows; no file over 1 MiB (images in `docs/assets` and fonts are allowed, see `.githooks/large-files.allow`); no merge-conflict markers and no trailing whitespace on lines you added; a secret scan (`gitleaks` if installed, otherwise built-in patterns for private keys, GitHub / AWS / Slack / Google / Stripe keys and file names such as `.env`, `*.pem`, `id_rsa`; end a line with `secret-scan:allow` for a false positive); the retired product names ([Names](#ground-rules); `crates/core/tests/names.rs` stays the authority); and a warning, not a block, when `user.email` is not a `users.noreply.github.com` address, because the e-mail of a commit is public |
| `pre-push` | `commit-lint.sh` on the commits being pushed, then `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` (with a throw-away `HOME`, like `scripts/check.sh`). `SKIP_PREPUSH=1 git push` skips the two cargo steps only; the message check still runs |

Prefer the [pre-commit](https://pre-commit.com) framework? `.pre-commit-config.yaml` calls the same scripts. Use one of the two:
`scripts/setup-hooks.sh --uninstall` first, then `pre-commit install --hook-type pre-commit --hook-type commit-msg --hook-type pre-push`.

You can check a message or a range yourself: `scripts/commit-lint.sh --title "feat(gui): add x"`, `scripts/commit-lint.sh --range origin/main..HEAD`.
To fix a bad message: `git commit --amend` for the last commit, `git rebase -i origin/main` (mark `reword`) for older ones, or squash the branch.

**CI repeats everything, because hooks can be skipped.** The `commit-lint` workflow validates the pull request title and every commit message
of the pull request; `check` runs `scripts/check.sh` (fmt, clippy, tests, and the tests of these hooks); the `packaging` job lints every
shell script and workflow; `cargo deny` and the name guard test run there too. **In an emergency** `git commit --no-verify` and
`git push --no-verify` skip the hooks, but the pull request will still be red until the title and the commits are fixed, so use them
only to save work-in-progress locally, never to get around a check.

## Pull requests

* Title in the Conventional Commits format (see above); the description is the template: what and why, how you tested it, and what you could not test (for example anything that needs a real keyboard,
  a second display or a permission prompt).
* Keep a pull request to one topic. `scripts/check.sh` must pass; add or update tests for behaviour changes and the matching
  page in `docs/` for anything a user can see.
* Mention a security impact in the description, but report an actual vulnerability privately first ([.github/SECURITY.md](.github/SECURITY.md)).

## Reporting problems

Use the issue templates in `.github/ISSUE_TEMPLATE/`. For security problems follow [.github/SECURITY.md](.github/SECURITY.md).

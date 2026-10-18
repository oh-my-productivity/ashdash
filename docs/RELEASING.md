# Releasing Ashdash

A release is one click: **Actions > release > Run workflow** (from `main`). The version number is computed from the
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) that landed on `main` since the previous release, the release commit
`chore(release): vX.Y.Z` and the annotated tag `vX.Y.Z` are pushed to `main`, and a **draft** GitHub release is created with the DMG, its
SHA-256 and generated notes. Nothing is public until a maintainer reads the draft and presses *Publish*.

The packaging scripts in `packaging/` take identities and credentials only from environment variables and fall back to **ad-hoc signing**, so the whole pipeline
runs (and is tested) without any Apple account; ad-hoc builds just are not distributable to other people without a Gatekeeper warning.

* [How to release](#how-to-release)
* [How the version is chosen](#how-the-version-is-chosen)
* [Tooling: why git-cliff](#tooling-why-git-cliff)
* [The workflow](#the-workflow) and [how the release commit gets past the protected main](#how-the-release-commit-gets-past-the-protected-main)
* [One-time setup](#one-time-setup) (for the repository admin)
* [Forcing a version, release candidates, 1.0](#forcing-a-version-release-candidates-10)
* [CHANGELOG.md and the release notes](#changelogmd-and-the-release-notes)
* [When a run fails](#when-a-run-fails)
* [The scripts](#the-scripts) and [packaging](#packaging-scripts)

## How to release

1. Look at what the next release would be. Every pull request and every push to `main` writes it into the job summary of the `release-preview` workflow
   (the version, the commits that decide it, the release notes as they would be written). Locally: `scripts/release/next-version.sh` (needs `git-cliff`, see below).
2. Run **release** with `dry_run` left on (the default). Read the plan in the job summary: version, reason, the files the release commit would change, the notes.
   Nothing is changed.
3. Run **release** again from `main` with `dry_run` switched off. The jobs: `plan` (about 1 minute), `build` (about 8 minutes, macOS), `publish` (about 1 minute).
4. Open the draft release, read the notes, download the DMG if you want to try it, press **Publish**.

Only a maintainer with write access can start the workflow; a real release only runs from `main` (any branch can do a dry run).
Two releases cannot overlap (`concurrency` group `release`, a running release is never cancelled).

## How the version is chosen

`scripts/release/next-version.sh` looks at the commits between the previous stable tag (`vX.Y.Z`, the highest one reachable from `main`) and `HEAD`.
Pull requests are squash-merged, so each commit on `main` is a pull request and its header is the pull request title (checked by `commit-lint`).
While the major version is **0** (now):

| Commit type | Section in the changelog | Effect on the version |
|---|---|---|
| `feat` | Added | **minor** (0.1.4 -> 0.2.0) |
| `fix`, `perf` | Fixed, Performance | **patch** (0.1.4 -> 0.1.5) |
| any type with `!` (`feat(gui)!: ...`) or a `BREAKING CHANGE:` footer | Breaking changes (and its own section) | **minor** in 0.x, **major** from 1.0.0 on |
| `refactor`, `revert` | Changed | none |
| `docs` | Documentation | none |
| `test`, `build`, `ci`, `chore`, `style` | Other | none |
| `chore(release): ...` | left out (the release commit itself) | none |
| anything that is not a Conventional Commit (a `Merge ...`, a `Revert "..."` written by git) | left out | none |

The biggest of the commits wins. If no commit is a `feat`, `fix`, `perf` or breaking change the answer is **nothing to release** (exit status 10; the workflow
says so and stops). A `revert:` alone does not release (an owner decision: only the types above release); if the revert must ship, write it as `fix: revert ...`.
From 1.0.0 on the table is the standard one: breaking is major, `feat` is minor, `fix`/`perf` is patch.

The first release is `v0.1.0`: there is no previous tag, so the version in `crates/app/Cargo.toml` (without a `-rc.N` suffix) is released as it is, and the old history is not parsed
(see [CHANGELOG.md and the release notes](#changelogmd-and-the-release-notes)).

The result is checked twice: `next-version.sh` applies the rules itself (from the commit groups `cliff.toml` produces) and compares with `git-cliff --bumped-version`;
a disagreement stops the release. Inputs it refuses (exit status 65): uncommitted changes in tracked files, a shallow clone (tags are missing), the version in `Cargo.toml` not equal to
the highest release tag (someone edited the version by hand), the two crates at different versions, a version that already exists or is not higher than the tags.

## Tooling: why git-cliff

Constraints: free; works for a two-crate workspace of `publish = false` crates (nothing goes to crates.io); no Node; downloaded binaries pinned by version and verified by SHA-256;
deterministic output; works with squash-merged conventional commits; no network for the computation.

| Candidate | What it would give us | Why not (or yes) |
|---|---|---|
| **git-cliff** (chosen) | one static binary; changelog, release notes and `--bumped-version` from one `cliff.toml`; works offline; the commit grouping is configuration, not code | it does not edit manifests, commit or tag, and it has no "nothing to release" notion (a `docs`-only history still bumps the patch): about 300 lines of shell around it (`scripts/release/`) add exactly that, with tests |
| cocogitto | conventional-commit linting, bump, changelog, tags | it wants to make the bump commit and the tag itself and to run hooks; here the commit must be built and verified in a separate job before it is pushed with the deploy key. Its 0.x rules and "nothing to release" would need the same wrapper |
| release-plz | release pull requests, changelog, crates.io publishing | built around publishing to crates.io and a release pull request, i.e. design (b) below; this project publishes a DMG, and the macOS build has to sit between "version known" and "tag pushed" |
| cargo-release | bumps manifests, commits, tags, pushes | takes the level from the caller, does not read commits, and is aimed at crates.io |
| a small script only | no dependency | we would re-implement conventional-commit parsing, grouping, changelog templates and link building |

The pinned version and checksums are in `scripts/release/tools.env`; `scripts/release/install-git-cliff.sh` downloads the archive, compares its SHA-256, and only then unpacks it
(macOS arm64 and Linux x86-64 static; any other platform: install the pinned version yourself). To update: change `tools.env` as described in its header.

## The workflow

`release.yml` (`workflow_dispatch` only; pushing a tag does nothing) has three jobs. Inputs: `bump` (`auto`, `patch`, `minor`, `major`), `version` (exact), `prerelease` (bool), `dry_run` (bool, **default true**).

| Job | Runner, token | What it does |
|---|---|---|
| `plan` | ubuntu, read-only | `scripts/release/ci-plan.sh`: computes the version (the inputs reach the script only through environment variables), writes the plan to the job summary, and rehearses `prepare.sh` (diff stat and notes in the summary). A dry run stops here |
| `build` | macOS 15, read-only | checks out exactly the planned commit; `prepare.sh`; commits `chore(release): vX.Y.Z` (identity below); `verify-release-commit.sh`; checks that the tag, the planned version and `crates/app/Cargo.toml` agree; self-test, signing and notarization when the secrets exist, universal build, DMG, checksum, Homebrew cask; `tests/lab/ci/verify-bundle.sh`; checks the `Info.plist` and `ashdash --version` say the release version; uploads the DMG, checksum, cask, notes and the release commit (a `git bundle`) |
| `publish` | ubuntu, `contents: write`, environment `release`, the **only job that sees the deploy key** | fetches the commit from the bundle and runs `verify-release-commit.sh` again (one parent = the planned commit, subject `chore(release): vX.Y.Z`, only `Cargo.toml`/`crates/*/Cargo.toml`/`Cargo.lock`/`CHANGELOG.md` changed, `Cargo.lock` only in `version` lines, both crates at the version); refuses if `main` moved since the plan; tags (annotated) and pushes commit and tag in **one atomic push**; creates the **draft** release with `--verify-tag`, the notes, and `--prerelease` plus a Gatekeeper banner when the build is not signed and notarized, or when the version is a release candidate |

What is tagged is exactly what was built: the commit that the `build` job made, built and verified is the object that `publish` pushes. The release commit and the tag use the identity
`Amin Alizadeh <64799918+achieveramin@users.noreply.github.com>` (never a personal address); set the repository variables `RELEASE_GIT_NAME` and `RELEASE_GIT_EMAIL` to change it.
Third-party actions are pinned by commit SHA, every job has a timeout, and no secret is ever echoed. The check that "the tag equals the version in `Cargo.toml`" is still made, in `build`.

`release-preview.yml` runs on every pull request to `main` and on every push to `main`, with a read-only token and no secrets, and writes to the **job summary only** (no comment, so no spam, and
the same on pull requests from forks):

* `preview`: for a pull request, the release before and after the squash commit that merging would add (this repository's squash message is the pull request title plus its description, so a
  `BREAKING CHANGE:` footer in the description counts); for `main`, the next release as things stand, with the notes. It checks out the **base** branch and runs its scripts; the pull request
  contributes only its title and description as text.
* `release-tests`: `scripts/release/test/run-tests.sh` (about 220 checks, below).

### How the release commit gets past the protected main

The ruleset on `main` requires pull requests and the checks `check` and `deny`. `GITHUB_TOKEN` cannot bypass a ruleset, and a push made with it does not start other workflows (so CI would never run on the release
commit). Three designs were evaluated:

* **(a) a write deploy key that is a bypass actor of the ruleset: chosen.** The release job pushes the release commit and the tag over SSH with the key. Proven in a sandbox repository
  (see [Sandbox proof](#sandbox-proof-2026-10-18)): with an empty bypass list the push is rejected (`GH013 ... Changes must be made through a pull request`); with the actor `{"actor_type": "DeployKey", "bypass_mode": "always"}` in the
  list the same push is accepted ("Bypassed rule violations"); the push starts `ci.yml` on `main` like any other push. Observed too: while the ruleset's bypass list contained the repository **admin role**, a write deploy key that an
  admin had created also bypassed, even without a `DeployKey` actor; the explicit actor is what makes the design independent of that, and lets the admin role be removed from the list later.
* (b) a release pull request opened by the workflow and merged by the owner: needs a second click and a PR created by `GITHUB_TOKEN` (which does not start the required checks, so they must be nudged by hand),
  and the DMG would be built before the commit exists on `main`. Not needed because (a) works.
* (c) tag-only releases with the version injected at build time and `Cargo.toml` bumped in a follow-up pull request: leaves `main` out of step with the latest tag by design, and breaks "the tag equals the version in
  `Cargo.toml`". Not needed.

Two limits of a deploy key, both observed: it cannot create or update files under `.github/workflows` (GitHub refuses with "without `workflow` scope"; the release commit never touches them, and
`verify-release-commit.sh` would refuse it if it did). Stored as a secret of the `release` environment (see below) it can only be read by a workflow run on `main`.

Safety of (a): the key can write to the repository, and as a bypass actor it can write to `main`. So (1) it is stored as a secret of the **environment `release`**, which is limited to the branch `main`
(a workflow on any other branch cannot read it; required reviewers can be added there for an extra approval step), (2) only `publish` uses it, and `publish` only runs reviewed scripts of `main`, (3) `publish` refuses anything
but a version-bump commit (`verify-release-commit.sh`), (4) the push is atomic and a fast-forward of the planned commit: it fails if `main` moved, and force-pushes and deletions stay forbidden by the ruleset (a bypass actor could
ignore that too, which is why the job never force-pushes). Repository admins can rotate the key at any time (see below). A deploy key is bound to one repository, so it does not give access to anything else.

**Contributors with a fork** can open pull requests; they get the `release-preview` summary (read-only, no secrets: GitHub does not pass secrets to workflows from forks) and the `release-tests` job. They cannot start the
release workflow (it needs write access) and cannot read the deploy key. Their pull request title decides what the squash commit says, so they should follow `CONTRIBUTING.md`.

## One-time setup

For the repository admin of `oh-my-productivity/ashdash`; needs `gh` logged in with the `repo` and `admin:public_key` scopes. Nothing here is done by the workflow.

```sh
REPO=oh-my-productivity/ashdash
KEYDIR="$(mktemp -d)"

# 1. A key pair that exists only for this. No passphrase (a workflow cannot type one); it stays on this machine for a minute.
ssh-keygen -q -t ed25519 -N "" -C "ashdash release workflow" -f "$KEYDIR/release"

# 2. Register the public half as a deploy key WITH write access.
gh repo deploy-key add "$KEYDIR/release.pub" -R "$REPO" --allow-write --title "release workflow"

# 3. The environment "release": only the branch main may use it. (Add required reviewers in Settings > Environments if you want an approval click.)
gh api -X PUT "repos/$REPO/environments/release" \
  -F 'deployment_branch_policy[protected_branches]=false' -F 'deployment_branch_policy[custom_branch_policies]=true'
gh api -X POST "repos/$REPO/environments/release/deployment-branch-policies" -f name=main -f type=branch

# 4. The private half becomes the environment secret RELEASE_DEPLOY_KEY, then the local copy is destroyed.
gh secret set RELEASE_DEPLOY_KEY --env release -R "$REPO" < "$KEYDIR/release"
rm -P "$KEYDIR/release" "$KEYDIR/release.pub" 2>/dev/null || rm -f "$KEYDIR/release" "$KEYDIR/release.pub"; rmdir "$KEYDIR"

# 5. Let deploy keys bypass the ruleset "main": add the actor, keep everything else as it is.
RULESET_ID="$(gh api "repos/$REPO/rulesets" --jq '.[] | select(.name == "main") | .id')"
RULESET_JSON="$(mktemp)"
gh api "repos/$REPO/rulesets/$RULESET_ID" \
  | jq '{name, target, enforcement, conditions, rules,
         bypass_actors: ((.bypass_actors | map(select(.actor_type != "DeployKey"))) + [{"actor_type": "DeployKey", "bypass_mode": "always"}])}' \
  > "$RULESET_JSON"
cat "$RULESET_JSON"        # look at it: the rules must be the ones you know, bypass_actors has the old entries plus DeployKey
gh api -X PUT "repos/$REPO/rulesets/$RULESET_ID" --input "$RULESET_JSON" --jq '.bypass_actors'
rm -f "$RULESET_JSON"
```

The bypass JSON (an entry of `bypass_actors`) is `{"actor_id": null, "actor_type": "DeployKey", "bypass_mode": "always"}`; GitHub fills in `actor_id: null`. Check it:
`gh api "repos/$REPO/rulesets/$RULESET_ID" --jq '.bypass_actors'` must list a `DeployKey` entry.

After both work packages of wave 10 are merged, also add the status check `commit-lint` to the required checks of the same ruleset (the commit-lint workflow's job name).

Optional repository variables: `RELEASE_GIT_NAME`, `RELEASE_GIT_EMAIL` (identity of the release commit and tag).

Rotate or revoke: `gh repo deploy-key list -R "$REPO"` then `gh repo deploy-key delete <id> -R "$REPO"`, and repeat steps 1, 2 and 4. Deleting the key at once removes the ability to release; it never affects
normal pull requests.

First real use: run the workflow with `dry_run` on (default), read the plan, then run it for real. The first release is `v0.1.0`.

## Forcing a version, release candidates, 1.0

| Wish | Inputs |
|---|---|
| let the commits decide | `bump: auto` (default), `version` empty |
| a bigger or smaller bump than the commits say | `bump: patch`, `minor` or `major` (a lower bump than the commits ask for is allowed, with a warning in the log) |
| an exact version | `version: 0.9.0` (must be higher than every existing tag; the `v` is optional) |
| **1.0.0** | `bump: major` while on 0.x, or `version: 1.0.0`. It is never computed from commits: moving to 1.0 is a decision about stability, taken by a person |
| a release candidate | `prerelease: true`: the computed version gets `-rc.N` (`0.4.0-rc.1`, then `-rc.2`, ...), the manifests carry that version, the GitHub release is marked pre-release. Or `version: 0.4.0-rc.1` |
| the final release after candidates | run again with `prerelease` off: the version is computed from the last **stable** tag, so it is `0.4.0` (the sections of the candidates in `CHANGELOG.md` are replaced by one `0.4.0` section). The candidate's commit may be promoted without new commits; if the candidate was forced with `bump`, force the final version the same way (`version: 0.4.0`) |

Accepted version shapes: `X.Y.Z` and `X.Y.Z-alpha.N`, `-beta.N`, `-rc.N`; nothing else is ever tagged.
Overrides still need at least one commit since the last release tag (a commit that is already released cannot be released again).
Policy for 1.0 and later: from 1.0.0 the rules are the standard ones (breaking = major), and removing or changing a documented command-line option, URL command, config key or workflow file format counts as breaking.

## CHANGELOG.md and the release notes

`CHANGELOG.md` keeps the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) layout and is **generated from now on**: `prepare.sh` writes one section per release from the commits (sections
*Breaking changes, Added, Fixed, Performance, Changed, Documentation, Other*, each entry with the scope in bold and links to the pull request and the commit) and puts it above the previous one. Do not edit generated sections: the release commit is made by the workflow, so a wrong entry is fixed in the section of a later release or by a normal pull request afterwards.

* **The `## [Unreleased]` section is now only a bucket for hand-written text.** Whatever is under `## [Unreleased]` (and under a draft heading `## [X.Y.Z] - unreleased`) when a release is prepared is folded into the new
  section: subsections with the same name are merged, others are kept after the generated ones. After a release the section is gone; add one again only if you need to say something that no commit can.
* **The first release (`v0.1.0`)**: history before the convention is not parsed. Its section is the hand-written `## [0.1.0] - unreleased (release candidate)` section plus the entries of `## [Unreleased]`, merged by the same
  rule, with the date filled in. Nothing has to be done by hand before the first release.
* **Release candidates** get their own section (`## [0.4.0-rc.1] - date`); the final release replaces all candidate sections of that version with one section covering everything since the last stable tag.
* **Release notes** (the text of the GitHub release, `dist/release-notes.md`): the hand-written introduction `docs/release-notes/vX.Y.Z.md` if it exists (relative links are rewritten to the tag), then "What's changed"
  (the section of the changelog, and a compare link to the previous tag), and, when there is no hand-written introduction, the install block `docs/release-notes/install.md` (`@VERSION@` and `@REPO_URL@` are filled in).
  `docs/release-notes/v0.1.0.md` stays as the hand-written introduction of `v0.1.0` (it already has the install steps and the known issues); for the first release the notes are that file only.
* Mentions (`@name`) in commit messages are wrapped in backticks, so release notes never ping anyone. The date is the day of the run (UTC); `CLIFF_RELEASE_DATE` fixes it for reproducible runs.

## When a run fails

Nothing is pushed before the last step of `publish`, so most failures leave no trace.

| Where it failed | State | What to do |
|---|---|---|
| `plan`: exit 65, message names the problem (dirty tree, shallow clone, tag/Cargo mismatch, version exists) | nothing changed | fix the cause (for a tag/Cargo mismatch: `crates/*/Cargo.toml` must equal the highest tag; restore it in a normal pull request) and run again |
| `plan`: "Nothing to release" | nothing changed | merge a `feat`/`fix`/`perf`, or force the release with `bump` or `version` |
| `build` (compile error, signing, `verify-bundle.sh`, version mismatch) | nothing pushed; the artifact may be missing | fix `main` with a normal pull request, run the workflow again (the version is recomputed) |
| `publish`: "The secret RELEASE_DEPLOY_KEY is not set" | nothing pushed | do the [one-time setup](#one-time-setup), then **Re-run failed jobs** (the build is kept for 14 days) |
| `publish`: "main moved on" | nothing pushed | someone merged while the build ran; run the whole workflow again (it is cheap to repeat; the version may change) |
| `publish`: `GH013 Repository rule violations` | nothing pushed | the deploy key is not a bypass actor, was deleted, or the ruleset changed: check `bypass_actors` and `gh repo deploy-key list`; then **Re-run failed jobs** |
| `publish`: the push worked, the release was not created | commit and tag are on `main` | **Re-run failed jobs**: the push step sees that `main` and the tag are already at the release commit and skips, the release step creates the draft (or re-uploads the files if the release exists) |
| the draft is wrong but commit and tag are fine | draft only | `gh release delete vX.Y.Z --yes` (keeps the tag), then re-run the `publish` job |
| the release itself is wrong after publishing | tag and commit are public | never move or delete a published tag. Fix `main`, release the next patch version; mark the bad release as pre-release or delete it by hand if it must disappear |
| a bad `chore(release)` commit reached `main` (should be impossible: the gate checks it) | `main` | revert it with a normal pull request, `fix: revert ...`, release again |

Drafts and tags are never created without a successful build, and a tag is never pushed without its commit (one atomic push).

## Local use

```sh
scripts/release/install-git-cliff.sh             # pinned git-cliff into ~/.cache/ashdash-release-tools; put the printed directory on PATH
scripts/release/next-version.sh                  # version on stdout, reason on stderr; exit 10 = nothing to release
scripts/release/next-version.sh --bump minor     # or --version 0.9.0, --prerelease rc, --reason-file FILE
scripts/release/preview.sh                       # Markdown: the plan and the notes, works in a temporary clone
scripts/release/test/run-tests.sh                # the tests (REQUIRE_GIT_CLIFF=1 makes a missing git-cliff an error)
```

`prepare.sh VERSION` changes your working tree (manifests, `Cargo.lock`, `CHANGELOG.md`, `dist/release-notes.md`); use it on a scratch clone, or `git checkout .` afterwards. It never runs `cargo update`.

## The scripts

All in `scripts/release/`, bash 3.2 compatible (macOS) and checked with shellcheck. Exit statuses: 0 fine, 10 nothing to release, 64 wrong arguments, 65 the repository or the inputs do not allow a release, 69 a tool is missing.

| Script | Purpose |
|---|---|
| `next-version.sh` | the next version and why; `--bump`, `--version`, `--prerelease`, `--reason-file`, `--allow-dirty` |
| `prepare.sh VERSION` | versions in the manifests (including the `version` of the path dependency between the crates) and in `Cargo.lock` (the two workspace packages only; checked with `cargo metadata --locked` and by diffing), the `CHANGELOG.md` section, `dist/release-notes.md` |
| `verify-release-commit.sh BASE COMMIT VERSION` | the gate in front of the deploy key |
| `ci-plan.sh` | the plan step of the workflow (inputs from the environment, outputs and summary) |
| `preview.sh [--message-file F]` | the report of `release-preview.yml` |
| `install-git-cliff.sh` | the pinned, checksummed installer |
| `changelog-merge.awk`, `lib.sh`, `tools.env` | helpers and the pins |
| `test/` | `test-next-version.sh` (85 checks on synthetic repositories: first release, only docs, fix, perf, feat, `feat!`, `BREAKING CHANGE` footers, revert, mixed, 1.x, overrides, candidates, dirty/shallow/mismatch, unreachable tags), `test-prepare.sh` (87), `test-ci.sh` (47) |

## Signing secrets

Repository secrets for signing and notarization (Settings > Secrets and variables > Actions). Without them the workflow still works and
publishes an **ad-hoc, un-notarized pre-release**.

| Secret | Content |
|---|---|
| `MACOS_CERT_P12_BASE64` | Developer ID Application certificate + private key exported from Keychain Access as `.p12`, then `base64 -i cert.p12` |
| `MACOS_CERT_PASSWORD` | the password you chose when exporting the `.p12` |
| `CODESIGN_IDENTITY` | the identity string, e.g. `Developer ID Application: Your Name (TEAMID1234)` (`security find-identity -v -p codesigning`) |
| `NOTARY_APPLE_ID` | Apple ID e-mail used for notarization |
| `NOTARY_TEAM_ID` | 10-character team id |
| `NOTARY_PASSWORD` | an **app-specific password** from appleid.apple.com (never the account password) |

Notarization needs all three `NOTARY_*` secrets *and* the certificate. The signing, keychain and notarization steps have never run (no Apple account yet).
Scheduled job `advisories.yml` (weekly, Monday 06:17 UTC, and on demand) runs `cargo deny check advisories`; a red run is the signal.
Known limits: the Rust toolchain is `stable` at the time of the run, not a pinned version; `macos-15` is the runner image GitHub offers today and may be retired.

## Packaging scripts

| Script | Purpose |
|---|---|
| `packaging/build-app.sh` | universal (arm64 + x86_64, joined with `lipo`) release build, assembles and signs `dist/Ashdash.app`, verifies it |
| `packaging/make-dmg.sh` | `dist/Ashdash-<version>.dmg` (app + `Applications` symlink) and its `.sha256` |
| `packaging/notarize.sh` | `notarytool submit --wait` + `stapler staple` for an `.app` or `.dmg`; a no-op without `NOTARY_PROFILE` |
| `packaging/homebrew/render-cask.sh` | renders the Homebrew cask for a release from the DMG's sha256 |
| `packaging/icon/make-icns.sh` | draws the icon with CoreGraphics (`make-icon.swift`) and packs it with `iconutil` |
| `packaging/selftest.sh` | offline checks of all of the above (plist, entitlements, cask, scripts); run by CI first |


## Local build (no Apple account needed)

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin   # once
packaging/selftest.sh                # seconds
packaging/build-app.sh               # ~2.5 min cold on an M3 Pro (see "Dry run"); writes dist/Ashdash.app, ad-hoc signed
packaging/make-dmg.sh                # dist/Ashdash-<version>.dmg
```

Useful switches: `ARCHS=native` (one architecture, about half the time), `OUT_DIR=/tmp/x`,
`BUILD_NUMBER=7`, `MIN_OS=13.0`, `SKIP_ICON=1`.

The version comes from `crates/app/Cargo.toml`; the bundle id, product name and URL scheme come from
`crates/core/src/brand.rs`. Nothing is duplicated there. Changing the slug or bundle id moves the user's data folder and makes macOS ask for permissions again; a product rename also
touches the Cargo package names, the scripting dictionary file name (`crates/app/resources/Ashdash.sdef`, which must match `OSAScriptingDefinition`), `Info.plist.template`, the packaging scripts and the docs.

What `build-app.sh` checks for you: `codesign --verify --deep --strict`, and it prints what `spctl` thinks.
`spctl` **rejects** ad-hoc and un-notarized bundles; the script reports that as a warning, not an error.

### Running an ad-hoc build on another Mac

Gatekeeper blocks it. On macOS 15 and later the dialog only offers *Done* / *Move to Trash* (verified in the lab on macOS 15.7): use System Settings > Privacy & Security > *Open Anyway* and confirm in the second dialog, or
`xattr -dr com.apple.quarantine /Applications/Ashdash.app`. Right-click > Open is the route up to macOS 14. This is expected for unsigned/ad-hoc software.

## Signing and notarization (real releases)

You need an Apple Developer Program membership and a **Developer ID Application** certificate.

Local, with your own keychain:

```sh
# one time: store notarization credentials in the keychain under a profile name
xcrun notarytool store-credentials ashdash-notary \
    --apple-id you@example.com --team-id TEAMID1234 --password <app-specific-password>

export CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID1234)"
export NOTARY_PROFILE=ashdash-notary
packaging/build-app.sh
packaging/notarize.sh dist/Ashdash.app     # staples the app
packaging/make-dmg.sh dist/Ashdash.app     # signs the DMG too
packaging/notarize.sh dist/Ashdash-0.1.0.dmg
```

The app is signed with the hardened runtime (`--options runtime`) and the entitlements in
`packaging/entitlements.plist`: only `com.apple.security.automation.apple-events` (so workflows and the system
commands may drive other apps with AppleScript). No JIT, no library-validation exemption, no sandbox: a launcher
must be able to open arbitrary apps and files. If a future feature needs more, add the single entitlement there and
say why in the commit message.

`notarize.sh` refuses an ad-hoc or non-hardened app before uploading, and prints Apple's log if the submission is
rejected.


## Homebrew

No tap is published by the pipeline. The release uploads a rendered `ashdash.rb` next to the DMG. To publish:

```sh
packaging/homebrew/render-cask.sh --dmg dist/Ashdash-0.2.0.dmg --repo oh-my-productivity/ashdash --output ashdash.rb
brew audit --cask --new ./ashdash.rb      # needs a real, reachable release URL
# copy ashdash.rb to Casks/ashdash.rb in your tap repository (homebrew-tap) and push it
```

`--repo` defaults to `$GITHUB_REPOSITORY` and then to the placeholder `oh-my-productivity/ashdash`. A cask for the official
`homebrew-cask` repository additionally needs a notarized app and a stable, notable project.

## Login item and file associations

* `ashdash login-item enable|disable|status` uses `SMAppService.mainApp`. It only works from the bundle
  (`/Applications/Ashdash.app` is the safe place; macOS 13+). From a bare binary it explains why and does nothing.
  The first time, macOS may show "Ashdash added items that can run in the background"; the user can revoke it in
  System Settings > General > Login Items.
* Double-clicking a workflow package, appearance file or snippet package ([file types](WORKFLOWS.md#file-formats-and-identifiers)) *with Ashdash as the handler*
  imports it (workflows ask for confirmation first). The bundle registers itself with `LSHandlerRank = Alternate`,
  so an application that already handles those files stays the default opener; choose Ashdash via Open With, or
  `open -a Ashdash.app <file>`.
* The URL scheme is `ashdash://` only; Ashdash registers no other scheme.

## Dry run, 2026-10-16

Machine: Apple M3 Pro, macOS 26.5.2, rustc 1.95.0, load average 5-16 (a busy desktop with another build running), version 0.1.0, ad-hoc signing, no network use by the scripts.

| Step | Result | Time |
|---|---|---|
| `packaging/selftest.sh` | all checks ok (with and without `shellcheck` on `PATH`, after the fix below) | 3 s |
| `ARCHS=native packaging/build-app.sh` | thin arm64, cold target dir for that triple (dependencies compiled) | 1 min 18 s |
| `packaging/make-dmg.sh` (native) | 8.8 MB DMG (8,835,559 bytes) | 11 s |
| `packaging/build-app.sh` (universal; arm64 slice reused from the native build) | x86_64 compiled cold, `lipo` joined | 1 min 8 s |
| `packaging/make-dmg.sh` (universal) | 17.7 MB DMG (17,700,269 bytes), `.sha256` written and re-checked with `shasum -c` | 8 s |

A cold universal build from scratch is therefore about 2.5 minutes here (about 1.2 min per architecture, 5-6 cores busy).

Sizes: main binary 16.3 MB (arm64, stripped, thin LTO), 34.1 MB universal; `Ashdash.app` 17 MB native, 34 MB universal; DMG 8.8 MB / 17.7 MB.

Checked on both builds (native, then universal):

* `codesign --verify --deep --strict` passes; `Signature=adhoc`, hardened runtime flag present (`flags=0x10002(adhoc,runtime)`), `TeamIdentifier=not set`;
* `lipo -archs` prints `arm64` (native) and `x86_64 arm64` (universal); `Format=app bundle with Mach-O universal (x86_64 arm64)`;
* `plutil -lint Contents/Info.plist` OK; `spctl --assess` rejects the bundle, as expected for ad-hoc and not notarized (the script treats it as a warning);
* the DMG: `hdiutil verify` VALID, `hdiutil attach -readonly -nobrowse` mounts it with `Ashdash.app` and the `Applications` symlink, writing to the volume fails ("Read-only file system"), the app inside still passes
  `codesign --verify --deep --strict`, and the volume was detached afterwards;
* with a temporary `HOME`: `Ashdash.app/Contents/MacOS/ashdash --help`, `paths` and `query calc` run and print sensible output. The x86_64 slice also ran: `arch -x86_64 .../ashdash query calc` under Rosetta printed the same results.
  So the "x86_64 slice untested" item is reduced to "not run on Intel hardware" (Rosetta is not a real Intel Mac).

Not done (on purpose or not possible): launching the bundle with `open` (opens a GUI), Gatekeeper behaviour of a quarantined download, notarization and stapling (no Apple credentials), `brew install --cask`.

Findings:

* `selftest.sh` passes `-P SCRIPTDIR` to `shellcheck`, so it works the same on machines with and without `shellcheck`; `dist/` (the build output) is ignored by git.
* `ashdash --version` / `-V` / `version` prints `ashdash <crate version>` and exits 0; the release smoke test can use it.
* `CFBundleVersion` defaults to the commit count (141 at the time of the dry run); CI passes `github.run_number` instead, so build numbers differ between local and CI builds. Harmless, worth knowing.

## Sandbox proof, 2026-10-18

Everything above was proven end to end in a scratch repository (copy of the code, the same `main` ruleset recreated from the real one, squash-only merges, stub `check`/`deny` jobs; deleted afterwards, with its key).
GitHub does not offer rulesets on private repositories of a free personal account, so the scratch repository had to be public while the proof ran. Results:

| Step | Inputs | Result |
|---|---|---|
| dry run | defaults | `0.1.0` (first release, no tag), plan and notes in the summary, nothing changed |
| real run | `bump: auto` | `v0.1.0`: release commit and annotated tag pushed to the protected main by the deploy key, `ci` and `release-preview` started by that push, draft release with DMG, checksum, cask and notes; build 7 min 40 s |
| `docs:` pull request, then a real run | `bump: auto` | "Nothing to release since v0.1.0", `build` and `publish` skipped, run green |
| `fix(core): ...` pull request | `bump: auto` | `0.1.1`, section *Fixed* (+ *Documentation*, *Other*), pull-request and commit links |
| `feat(gui): ...` pull request | `bump: auto` | `0.2.0`, section *Added* |
| `feat(prefs)!: ...` with a `BREAKING CHANGE:` footer in the description | `bump: auto` | `0.3.0`, sections *Breaking changes* and *Added* (0.x: breaking is minor) |
| a pull request merged while the build ran | `bump: minor`, `prerelease: true` | `publish` refused: "main moved on ... Nothing was pushed"; no tag, no draft |
| the same inputs again | | `0.4.0-rc.1`, manifests at `0.4.0-rc.1`, release marked pre-release |
| plan only | `version: 1.0.0` | `1.0.0` ("this is the 1.0.0 decision") |
| promote the candidate | `version: 0.4.0` | `0.4.0`, the candidate's changelog section replaced by one `0.4.0` section |
| manual override | `bump: patch` | `0.4.1`; re-running only the `publish` job: "already at ... nothing to push", files uploaded again, no second tag |

Observed about the ruleset: a write deploy key pushing to `main` is rejected when the bypass list is empty (`GH013 ... Changes must be made through a pull request`), accepted with the `DeployKey` actor
("Bypassed rule violations"), and also accepted while the list had the repository admin role (the key had been created by an admin). `GITHUB_TOKEN` pushing an empty commit to `main` was rejected the same way.
A run on a branch other than `main` could not use the `release` environment ("Branch ... is not allowed to deploy to release"), and a real release dispatched from a branch fails in `plan`.
The commands of [One-time setup](#one-time-setup) were run verbatim against the scratch repository (only `REPO` changed).

Not proven: signing and notarization (no Apple account), the workflow on the real repository (its `release` environment, its secret and its ruleset do not exist yet), a release approval by a required reviewer,
the macOS 14/26 runners, and the draft being published.

## Not covered by automation

* Notarization and stapling (needs Apple credentials): untested until a maintainer runs a real release.
* Gatekeeper behaviour of a downloaded, quarantined build on a clean Mac.
* Intel hardware: the x86_64 slice is cross-compiled and signed and was run under Rosetta (`arch -x86_64`), not on an Intel Mac.
* Cask installation (`brew install --cask`) until a tap exists.
* The release workflow on the real repository: it was proven in a scratch repository ([Sandbox proof](#sandbox-proof-2026-10-18)), but the real deploy key, environment and ruleset change are one-time steps for the admin, and the signing and notarization steps have never run.

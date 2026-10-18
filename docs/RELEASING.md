# Releasing Ashdash

Everything lives in `packaging/` and `.github/workflows/release.yml`. The scripts take identities and
credentials only from environment variables and fall back to **ad-hoc signing**, so the whole pipeline
runs (and is tested) without any Apple account; ad-hoc builds just are not distributable to other people
without a Gatekeeper warning.

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
`crates/core/src/brand.rs`. Nothing is duplicated there. A rename also touches the Cargo package names, the scripting dictionary file name, `Info.plist.template`
and the docs (list: [CLEAN_ROOM.md](CLEAN_ROOM.md#where-the-name-lives)).

What `build-app.sh` checks for you: `codesign --verify --deep --strict`, and it prints what `spctl` thinks.
`spctl` **rejects** ad-hoc and un-notarized bundles; the script reports that as a warning, not an error.

### Running an ad-hoc build on another Mac

Gatekeeper blocks it. On macOS 15 and later the dialog only offers *Done* / *Move to Trash* (verified in the lab, `docs/qa-runs/2026-10-rc1.md` PKG-06..PKG-14): use System Settings > Privacy & Security > *Open Anyway* and confirm in the second dialog, or
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

## GitHub Actions release

Pushing a tag `vX.Y.Z` runs `release.yml` (all third-party actions are pinned by commit SHA; the workflow was checked with `actionlint` 1.7.12 and `shellcheck` 0.11.0):

Job `build` (`macos-14`, read-only token):

1. checks that the tag equals `crates/app/Cargo.toml`'s version (so `v0.1.0-rc1` style tags are rejected; the crate version has no suffix),
2. for tags: checks that `docs/release-notes/<tag>.md` exists and `CHANGELOG.md` has a `## [X.Y.Z]` section,
3. runs `packaging/selftest.sh`,
4. imports the certificate into a throwaway keychain (only if the secrets exist),
5. builds the universal app, notarizes it (a no-op without `NOTARY_PROFILE`), builds the DMG, notarizes it,
6. recomputes the sha256 (stapling changes the file), renders `ashdash.rb`,
7. uploads `dist/*.dmg`, `*.sha256` and `ashdash.rb` as a workflow artifact.

Job `publish` (`ubuntu-latest`, tags only, the only job with `contents: write`, runs no repository scripts): downloads the artifact and creates a **draft** release
with `--verify-tag`, the notes from `docs/release-notes/<tag>.md`, and `--prerelease` plus a Gatekeeper banner on top of the notes when the build is not signed and notarized.
Nothing is public until a maintainer reads the draft and presses Publish.

Scheduled job `advisories.yml` (weekly, Monday 06:17 UTC, and on demand) runs `cargo deny check advisories`; it opens no issues, a red run is the signal. The pull-request job `deny` in `ci.yml` checks
`licenses bans sources` only, so a new RustSec entry does not block unrelated pull requests.

Known limits of the workflow (not testable without a tag push and secrets): the signing, keychain and notarization steps have never run; a tag can be pushed from any branch (the workflow does not
check that the tagged commit is on `main`); the Rust toolchain is `stable` at the time of the run, not a pinned version; `macos-14` is the runner image GitHub offers today and may be retired.

Repository secrets (Settings > Secrets and variables > Actions). Without them the workflow still works and
publishes an **ad-hoc, un-notarized pre-release**.

| Secret | Content |
|---|---|
| `MACOS_CERT_P12_BASE64` | Developer ID Application certificate + private key exported from Keychain Access as `.p12`, then `base64 -i cert.p12` |
| `MACOS_CERT_PASSWORD` | the password you chose when exporting the `.p12` |
| `CODESIGN_IDENTITY` | the identity string, e.g. `Developer ID Application: Your Name (TEAMID1234)` (`security find-identity -v -p codesigning`) |
| `NOTARY_APPLE_ID` | Apple ID e-mail used for notarization |
| `NOTARY_TEAM_ID` | 10-character team id |
| `NOTARY_PASSWORD` | an **app-specific password** from appleid.apple.com (never the account password) |

Notarization needs all three `NOTARY_*` secrets *and* the certificate. A manual `workflow_dispatch` run builds and
uploads workflow artifacts but never creates a release.

To cut a release:

```sh
# 1. bump version in crates/app/Cargo.toml (and Cargo.lock), add a CHANGELOG.md section and docs/release-notes/vX.Y.Z.md
scripts/check.sh && packaging/selftest.sh
git commit -am "Release 0.2.0" && git tag v0.2.0 && git push origin main v0.2.0
```

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
* Double-clicking a `.alfredworkflow`/`.alfredappearance`/`.alfredsnippets` file *with Ashdash as the handler*
  imports it (workflows ask for confirmation first). The bundle registers itself with `LSHandlerRank = Alternate`,
  so a real reference launcher installation stays the default opener; choose Ashdash via Open With, or
  `open -a Ashdash.app file.alfredworkflow`.
* The URL scheme is `ashdash://` only. `reference://` belongs to the reference launcher and is deliberately not registered.

## Dry run, 2026-10-16 (W4-REL)

Machine: Apple M3 Pro, macOS 26.5.2, rustc 1.95.0, load average 5-16 (a busy desktop with another build running), `main` + Wave 4 branch, version 0.1.0, ad-hoc signing, no network use by the scripts.

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

* **Fixed:** `selftest.sh` reported `FAIL shellcheck` whenever `shellcheck` is installed (as on most CI images and developer machines), because the `# shellcheck source=lib.sh` hints were resolved relative to the working directory
  (SC1091); it now passes `-P SCRIPTDIR`. Before the fix the self-test passed only on machines without `shellcheck`, so this would have failed the release workflow's first step on a runner that has it.
* **Fixed:** `dist/` (the build output) was not in `.gitignore`.
* **Fixed in Wave 4 review (W4-R2):** `ashdash --version` / `-V` / `version` now prints `ashdash <crate version>` and exits 0 (it used to print "unknown command"). The release smoke test can use it.
* `CFBundleVersion` defaults to the commit count (141 in this worktree); CI passes `github.run_number` instead, so build numbers differ between local and CI builds. Harmless, worth knowing.

## Not covered by automation

* Notarization and stapling (needs Apple credentials): untested until a maintainer runs a real release.
* Gatekeeper behaviour of a downloaded, quarantined build on a clean Mac.
* Intel hardware: the x86_64 slice is cross-compiled and signed and was run under Rosetta (`arch -x86_64`), not on an Intel Mac.
* Cask installation (`brew install --cask`) until a tap exists.
* The release workflow itself (it needs a tag push; syntax and shell snippets are linted with `actionlint`/`shellcheck`, nothing more).

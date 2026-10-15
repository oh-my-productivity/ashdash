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
packaging/build-app.sh               # ~3 min cold; writes dist/Ashdash.app, ad-hoc signed
packaging/make-dmg.sh                # dist/Ashdash-<version>.dmg
```

Useful switches: `ARCHS=native` (one architecture, about half the time), `OUT_DIR=/tmp/x`,
`BUILD_NUMBER=7`, `MIN_OS=13.0`, `SKIP_ICON=1`.

The version comes from `crates/app/Cargo.toml`; the bundle id, product name and URL scheme come from
`crates/core/src/brand.rs`. Nothing is duplicated, so a rename (docs/PLAN.md WP12) is a one-file change plus
`Info.plist.template` if you want different copy.

What `build-app.sh` checks for you: `codesign --verify --deep --strict`, and it prints what `spctl` thinks.
`spctl` **rejects** ad-hoc and un-notarized bundles; the script reports that as a warning, not an error.

### Running an ad-hoc build on another Mac

Gatekeeper blocks it. Either right-click the app and choose Open once, or
`xattr -dr com.apple.quarantine /Applications/Ashdash.app`. This is expected for unsigned/ad-hoc software.

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

Pushing a tag `vX.Y.Z` runs `release.yml` on `macos-14`:

1. checks that the tag equals `crates/app/Cargo.toml`'s version,
2. runs `packaging/selftest.sh`,
3. imports the certificate into a throwaway keychain (only if the secrets exist),
4. builds the universal app, notarizes it, builds the DMG, notarizes it,
5. recomputes the sha256 (stapling changes the file), renders `ashdash.rb`,
6. uploads the artifacts and creates the GitHub release (`--prerelease` and a warning in the notes when the
   build is not signed and notarized).

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
# 1. bump version in crates/app/Cargo.toml (and Cargo.lock), update the changelog/notes
scripts/check.sh && packaging/selftest.sh
git commit -am "Release 0.2.0" && git tag v0.2.0 && git push origin main v0.2.0
```

## Homebrew

No tap is published by the pipeline. The release uploads a rendered `ashdash.rb` next to the DMG. To publish:

```sh
packaging/homebrew/render-cask.sh --dmg dist/Ashdash-0.2.0.dmg --repo OWNER/ashdash --output ashdash.rb
brew audit --cask --new ./ashdash.rb      # needs a real, reachable release URL
# copy ashdash.rb to Casks/ashdash.rb in your tap repository (homebrew-tap) and push it
```

`--repo` defaults to `$GITHUB_REPOSITORY` and then to the placeholder `OWNER/ashdash`. A cask for the official
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

## Not covered by automation

* Notarization and stapling (needs Apple credentials): untested until a maintainer runs a real release.
* Gatekeeper behaviour of a downloaded, quarantined build on a clean Mac.
* Intel hardware: the x86_64 slice is cross-compiled and signed but only runs on an Intel Mac or under Rosetta.
* Cask installation (`brew install --cask`) until a tap exists.

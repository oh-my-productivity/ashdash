# Continuous integration

Everything that can run without a physical Mac runs on free GitHub-hosted runners (the repository is public, and standard hosted runners
are free for public repositories, macOS included). The rest is listed in [What needs a physical Mac](#what-needs-a-physical-mac).

| Workflow | Runs on | Jobs |
|---|---|---|
| `ci.yml` | every pull request and push to `main`; `workflow_dispatch` | `check`, `deny`, `lab-scripts`, `packaging` |
| `ci.yml` | pushes to `main` and manual runs only | `gui-smoke` (macOS 15 and 26), `x86-64` (macOS 15 Intel), `bundle` |
| `nightly.yml` | every night 02:41 UTC; `workflow_dispatch` | `checklist` (macOS 15 and 26), `soak` (macOS 15) |
| `advisories.yml` | Mondays 06:17 UTC; `workflow_dispatch` | RustSec advisories for the locked dependencies |
| `release.yml` | a pushed `v*` tag; `workflow_dispatch` builds and uploads but never creates a release | signed or ad-hoc universal build, draft release |

Required status checks on `main`: `check` and `deny`.

## What each job does

* **check** (macos-15): `scripts/check.sh` = `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`.
* **deny** (ubuntu): `cargo deny check licenses bans sources` (`deny.toml`). Advisories are deliberately not part of pull requests: a new
  RustSec entry for a crate a change does not touch would turn every pull request red. They run weekly in `advisories.yml`.
* **lab-scripts** (macos-15): `tests/lab/test/run-tests.sh`, the host tests of the lab scripts against a fake `tart` and `ssh`: slot
  semaphore, `lab-with-vm`, the guards of `tests/lab/guest/tcc-grant.sh` (including the CI mode) and of the CI wrappers, `finalize.py`.
* **packaging** (macos-15): `packaging/selftest.sh`, `scripts/gen-notice.py --check` (NOTICE is up to date), and `tests/lab/ci/lint.sh`:
  shellcheck (warnings and errors) on every shell script in the repository and actionlint on the workflows. Both linters are downloaded as
  pinned releases and verified against a SHA-256 in the workflow.
* **gui-smoke** (macos-15, macos-26): `tests/lab/ci/run-smoke.sh` = the lab's `tests/lab/smoke.sh --quick` on the runner: debug build, native
  `Ashdash.app` with `packaging/build-app.sh`, launch in the GUI session, `ashdash show/search/query`, the `ashdash://` scheme, the global
  hotkey Option+Shift+Space through System Events (the window must toggle), window state read back as text, a screenshot, login item
  enable/status/disable, `quit`. `scripts/check.sh` is not repeated (the `check` job does it).
* **x86-64** (macos-15-intel): `cargo test --workspace`, `cargo build -p ashdash` and a native x86-64 bundle. **GitHub retires the
  `macos-15-intel` image in August 2027**, the last Intel hosted runner; after that there is no free Intel CI. Plan: keep the
  universal build in `bundle` (it cross-compiles the x86-64 slice on arm64, but does not run it) and test on a physical Intel Mac or
  under Rosetta-less emulation is not possible, so x86-64 test coverage ends then. Remove this job when the label disappears.
* **bundle** (macos-15): `packaging/build-app.sh` (universal arm64 + x86_64, ad-hoc signed) and `packaging/make-dmg.sh`, then
  `tests/lab/ci/verify-bundle.sh`: both architectures present, `codesign --verify --deep --strict`, `plutil -lint`, the DMG checksum and
  `hdiutil verify`, the app inside the mounted image, and the bundled binary starts. The DMG and its checksum are uploaded as a workflow
  artifact for 7 days (not a release).
* **checklist** (nightly): `tests/lab/ci/run-checklist.sh` = `qa/prep.sh` (grants, native bundle, install to `/Applications`) +
  `qa/checklist.sh` (all sections: hotkey and window, search, keys, focus, clipboard and snippets, workflows, permissions, preferences,
  actions panel, measurements, the later behaviours) + `tests/lab/ci/finalize.py`. About 140 rows; a run takes about 22 minutes.
* **soak** (nightly): `tests/lab/ci/run-soak.sh` = `qa/soak.sh` for 7200 s (the 2-hour version of the lab): show, type, hide every 30 s,
  sample RSS, footprint, CPU time, threads and open files every 5 minutes, count crash reports, check the hotkey at the end. The dispatch
  input `soak_seconds` shortens it. A hosted job may run for up to 6 hours; the job limit here is 5.

Logs, results and screenshots are uploaded as artifacts **only when a job fails** (7 days). Screenshots of green runs are deleted before the
job ends. The job summary page of every run has the report (smoke stages, checklist rows that are not PASS, soak samples).

## Pins

Third-party actions are pinned by commit SHA; the comment on each line names the release. The newest stable releases when this was written
(2026-10-18):

| Action | Release | Commit |
|---|---|---|
| `actions/checkout` | v7.0.1 | `3d3c42e5aac5ba805825da76410c181273ba90b1` |
| `actions/upload-artifact` | v7.0.2 | `cf430e030ddbb5b0abf93d22962f4752f3646cd9` |
| `actions/download-artifact` | v8.0.2 | `9000827ccba6bdab643e8b6fd33ac0654aef8333` |
| `Swatinem/rust-cache` | v2.9.2 | `6323deb102c322ba6fcbdcafc7e3dddab59af2b6` |
| `EmbarkStudios/cargo-deny-action` | v2.1.1 | `3c6349835b2b7b196a839186cb8b78e02f7b5f25` |
| `dtolnay/rust-toolchain` | branch `stable` | `b725eda525362e7cf34af9a7243cac1e3f01a7ec` (the project publishes no release tags for it; Dependabot moves the SHA) |
| shellcheck (downloaded) | v0.11.0 | tarball SHA-256 in `ci.yml` |
| actionlint (downloaded) | v1.7.12 | tarball SHA-256 in `ci.yml` |

`rust-cache` and `cargo-deny-action` use annotated tags: the pinned value is the commit the tag points at, not the tag object.
Dependabot (`.github/dependabot.yml`) proposes updates weekly.

Security rules for the public repository: workflows run on `pull_request` (never `pull_request_target`), the token is read-only
(`permissions: contents: read`; only the draft-release step of `release.yml` gets `contents: write`), there are no secrets in the pull
request workflows, checkouts do not persist credentials, every job has a `timeout-minutes`, and a `concurrency` group cancels superseded runs.

## What a hosted runner is, and how it differs from the lab VM

A spike on 2026-10-18 (all three images, same commit) established that a hosted macOS runner is the same kind of machine as a lab clone:

| | `macos-15` | `macos-26` | `macos-15-intel` |
|---|---|---|---|
| macOS | 15.7.9 (24G830) | 26.6.2 (25G83) | 15.7.9 (24G830) |
| Architecture, model | arm64, `VirtualMac2,1` (Apple virtualization) | arm64, `VirtualMac2,1` | x86_64, `Macmini6,2` (a hypervisor, not Apple's) |
| CPUs, memory | 3, 7 GB | 3, 7 GB | 4, 14 GB |
| Display | 1024 x 768 | 1024 x 768 | 1920 x 1080 |
| SIP | disabled | disabled | disabled |
| Account, console | `runner`, logged in on `/dev/console`, session type Aqua | same | same |
| Passwordless `sudo` | yes | yes | yes |
| System Events (`osascript ... keystroke`) | works, no consent dialog | works | works |
| `screencapture -x` | real pixels (Screen Recording granted) | real pixels | real pixels |
| `CGWindowListCopyWindowInfo` (`scripts/qa/windows.swift`) | lists menu bar, Dock, Finder and the launcher windows | same | same |
| TCC databases | readable and writable with `sudo`; Accessibility and AppleEvents are pre-granted to `/bin/bash`, `osascript`, `sshd-keygen-wrapper`, Terminal | same | same |
| `cargo build -p ashdash` + `packaging/build-app.sh` (native) | yes (4 min 25 s cold) | yes (4 min 10 s cold) | yes (10 min 27 s cold) |
| Launch the bundle in the GUI session | yes, window visible | yes | yes |
| Global hotkey through System Events | toggles the launcher (1 window -> 0) | same | same |
| `ashdash://` through `open` | handler registered with `lsregister`, `open` returns 0 | same | same |
| `tests/lab/guest/tcc-grant.sh` (CI mode) | accepted (VirtualMac, user `runner`) | accepted | refused: the model is not `VirtualMac*`; not needed, the grants exist already |
| Spotlight | on, indexes slowly | on | on |

Differences from the lab VM (macOS 15 on Tart) that matter for results:

* **Spotlight is on** here and off in the lab. Rows that the lab marks BLOCKED because `mdfind` is empty (3.2.8c, 3.9.2, 3.11.3) can run,
  but a file created seconds ago is not indexed yet: `checklist.sh` waits up to 60 s for its fixture before asking.
* **Smaller display** (1024 x 768 on arm64). Any check that looks for a window of at least a given size sees the launcher's hidden full-size window
  as well. See the two open rows below.
* **Three cores, 7 GB**, shared hardware: timings are noisier than on the lab host. A unit test that slept for 30 ms and required the clock
  not to pass 80 ms failed on every arm64 runner; it was loosened (see "Findings while bringing CI up").
* **Pre-granted privacy permissions.** The hosted image grants Accessibility, Automation and Screen Recording to the tools the scripts use, so
  the first-use consent dialogs the lab sees do not appear. `tcc-grant.sh` still runs (it adds the Terminal and `osascript` rows the
  scripts rely on), and the "no TCC rows for the app" paths are as exercised as in the lab: nothing is granted to Ashdash itself.
* **Image content.** Firefox, Chrome, Edge and PowerShell are present as bundles, Visual Studio Code is not (3.2.1b stays BLOCKED),
  and `/Applications/Safari.app` is a symlink into a cryptex.
* The runner is thrown away after the job: nothing the scripts write (login item, TCC rows, `/Applications/Ashdash.app`) survives.

The scripts that drive the keyboard, the screen and the hotkey (`run-smoke.sh`, `run-checklist.sh`, `run-soak.sh`) refuse to run unless
`GITHUB_ACTIONS=true`, `RUNNER_ENVIRONMENT=github-hosted`, the account is `runner` and it owns the console. `tcc-grant.sh` additionally
requires a `VirtualMac*` model, so exporting the variables on a developer's Mac changes nothing (host tests: `tests/lab/test/run-tests.sh`).

## Rows that differ or cannot run: `tests/lab/ci/blocked.tsv`

`finalize.py` turns a **FAIL** into **BLOCKED** with the reason only for rows listed in `tests/lab/ci/blocked.tsv` (optionally for one
image); a PASS stays a PASS and every other FAIL fails the job. The BLOCKED and NEEDS-OWNER rows that `checklist.sh` itself reports are the
same ones as in the lab (no second display, display sleep locks the session, restart/logout/lock would end the session, no network switch,
Full Disk Access is not enforced with SIP off, nothing to type with an IME, ...).

RESULTS_PLACEHOLDER

## Reproducing a CI failure locally

1. Download the artifact of the failed job (`gh run download <run-id> -n <name>`): `smoke-report.txt` / `results.tsv` / `results-final.tsv`,
   the logs and the screenshots of the failed steps.
2. Run the same script in a lab VM (see [LAB.md](LAB.md)); the CI wrappers are thin: they set the artifact folder, call the lab script and
   post-process the results.

   ```sh
   vm=$(tests/lab/lab-up --id repro --max-age 14400 | cut -d' ' -f1)
   tests/lab/lab-run "$vm" -- bash tests/lab/smoke.sh --quick                 # what gui-smoke runs
   tests/lab/lab-run "$vm" -- bash tests/lab/qa/prep.sh                       # what run-checklist.sh runs, step 1
   tests/lab/lab-run "$vm" -- bash tests/lab/qa/checklist.sh s31 s32          # a few sections only
   tests/lab/lab-down "$vm"
   ```
3. `workflow_dispatch` runs a workflow from any branch of this repository: `gh workflow run ci.yml --ref <branch>`. (A *new* workflow file
   can only be dispatched once it exists on `main`; until then trigger it with a pull request or a temporary `push` trigger on a scratch branch.)
   `nightly.yml` takes `soak_seconds` and `sections` as inputs: `gh workflow run nightly.yml -f soak_seconds=600 -f sections="s31 s32"`.
4. The hosted-runner specifics (screen size, Spotlight, image content) cannot be reproduced in the Tart lab. To see the real thing, dispatch
   the workflow, or run `tests/lab/ci/lint.sh` and `tests/lab/test/run-tests.sh` on your Mac for the parts that need no GUI.

## What needs a physical Mac

Unchanged from [QA.md section 4](QA.md#4-checks-that-need-a-physical-mac): multiple displays and Spaces, IME composition, real sleep and
display sleep, lock and unlock, display hot-plug, a non-Retina external display, a real password manager, Full Disk Access with SIP on,
the Gatekeeper first-launch dialogs for a quarantined download in Safari, notarization with a real Developer ID certificate, real-hardware
performance numbers ([PERF.md](PERF.md); a shared 3-core VM says nothing about start-up time or idle memory on a Mac), and Intel
hardware after August 2027.

## Cost model, forks and capacity

* Standard GitHub-hosted runners (all labels used here are standard) are free and not metered for public repositories. If the repository
  were made private, macOS minutes would be billed at a multiple of the Linux rate and the nightly soak alone would use about 250 macOS
  minutes a day; at that point move `nightly.yml` to a weekly schedule.
* A pull request from a fork runs `ci.yml` with a read-only token and no secrets, on this repository's runner capacity; the first
  workflow run of a new contributor needs a maintainer's approval. It runs only the cheap jobs (`check`, `deny`, `lab-scripts`,
  `packaging`); the GUI jobs, `x86-64`, `bundle` and the nightly workflow never run for a fork's pull request.
* In a fork, scheduled workflows are disabled until the fork owner enables them, and a push to the fork's `main` runs the push jobs on
  the fork owner's account: free if the fork is public, metered if it is private.
* **Capacity.** A free account may run 20 jobs at once, at most 5 of them on macOS ([Actions limits](https://docs.github.com/en/actions/reference/limits)).
  A push to `main` starts 7 macOS jobs and the nightly workflow 3, so some wait in the queue; the nightly schedule is at a time with no
  pushes expected. Cache storage is limited to 10 GB per repository and artifact storage to 500 MB on a free organisation account:
  artifacts are kept for 7 days and only the bundle and failures are uploaded.
* Superseded runs are cancelled (`concurrency`), so a quick series of pushes costs one run, not many.

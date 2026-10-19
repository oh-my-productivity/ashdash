# Testing policy

The rule, decided by the project owner on 2026-10-18:

1. **Only unit tests and integration tests run on a developer's machine** (and in the local git hooks).
   **Everything else runs on the CI pipeline**: end-to-end tests (anything that starts the app, shows a window, uses the hotkey, the URL
   scheme, Accessibility or Automation, screenshots, Quick Look, the pasteboard or Finder), the scripted QA checklist, soak tests,
   packaging and universal builds, cross-architecture runs, benchmarks and performance measurements, fuzzing at scale.
2. **Stress tests run on CI too.** Anything that stresses local resources runs on the pipeline: many CPU-bound loops, thousands of
   iterations, large data sets, long runs, many processes or virtual machines, timing and latency assertions.
3. **The local machine stays quiet.** `scripts/check.sh` has a hard time budget, uses no GUI, no virtual machine and no network except
   crates.io for the dependencies. The local VM lab (Tart) does not exist any more: to run an end-to-end, stress or performance suite you
   push your branch and run the workflow with `scripts/ci-run.sh`, then read the run and its artifacts.

## The two tiers

| | Tier 1: your machine | Tier 2: CI |
|---|---|---|
| What | unit tests, integration tests (`cargo test --workspace`), format, lint, the fast script tests, the policy check | end-to-end, the QA checklist, soak, stress, timing under load, performance, benchmarks, packaging, other macOS versions and Intel |
| How to run | `scripts/check.sh` | `scripts/ci-run.sh <suite>`; automatically on pull requests (`ci.yml`, `e2e.yml` smoke) |
| Budget | 90 s target on a laptop with a warm cache, **180 s hard limit** (the script fails above it) | a timeout per job; the long suites run nightly or on demand |
| Allowed | temp directories, local unix sockets and loopback, the file system watcher, child processes such as `sh` | the real keyboard, screen, hotkey, pasteboard, Finder, osascript, network, all CPUs |
| Forbidden | the list in [What the policy check rejects](#what-the-policy-check-rejects) | (nothing, but see [Security](#security-of-the-workflows)) |

### What belongs where

A test runs on your machine when it is fast, deterministic and independent of the machine around it. It is **CI-only** when any of these is true:

* it needs more than about **2 s** alone (measure it: [How tests are classified](#how-tests-are-classified));
* it loops over **1000 or more items**, or builds a data set of **5000 items** (or a buffer of 64 MiB, or a string of a million characters);
* it asserts on **wall-clock time**, or sleeps more than **200 ms** in total (a bound that only catches a hang, 5 s or more, is allowed);
* it does **real GUI or OS interaction**: starts the app, shows a window, posts key events, writes the pasteboard, runs `osascript`, `screencapture`, `open`, Quick Look, Finder automation, Spotlight;
* it needs the **network**;
* it is a **fuzz or property test with a large case count** (a small smoke count stays local).

Do not delete coverage to make a test fast: keep a fast variant on your machine where the test has regression value, and the full one on CI.

## Running things

### On your machine

```sh
scripts/check.sh                    # fmt, clippy, unit and integration tests, commit-lint tests, policy check, policy tests; prints every stage's time
cargo test -p ashdash-core          # one crate while you work
scripts/policy-check.sh             # only the policy check (a second)
tests/hooks/run-tests.sh            # the git hook tests (about 40 s of real commits; CI runs them, the gate does not)
```

`scripts/check.sh` ends with a table of the stage times and fails when the total passes 180 s, naming that budget and pointing here. A stage
that grows the gate past the target moves to CI. The pre-push hook applies the same budget to its clippy and test steps.
`CHECK_BUDGET_SECONDS` can be raised on CI only (a cold build cache on a hosted runner is slower than a laptop); on a laptop the script refuses it.

### On CI: `scripts/ci-run.sh`

```sh
git push -u origin HEAD                         # the branch must be on origin
scripts/ci-run.sh smoke --watch                 # GUI smoke test, macOS 15 and 26: starts the app, hotkey, URL scheme, quit
scripts/ci-run.sh checklist --sections "s31 s32" --watch --download out    # part of the scripted QA checklist; artifacts into out/
scripts/ci-run.sh screenshots --watch --download shots                    # the preferences window, every page, dark and light
scripts/ci-run.sh launcher-look --watch --download look                   # the launcher window, empty and with results
scripts/ci-run.sh ignored --watch               # the #[ignore = "ci: ..."] Rust tests in release mode + full fuzz counts
scripts/ci-run.sh timing --runs 30 --watch      # clock-dependent tests under CPU load, many times
scripts/ci-run.sh perf-gui --watch --download perf   # start-up, hotkey latency, idle CPU and memory of the real app
scripts/ci-run.sh bench --watch                 # benchmarks
scripts/ci-run.sh soak --soak-seconds 600 --watch   # the soak, shortened
scripts/ci-run.sh smoke --dry-run               # show the gh command, do nothing
```

`ci-run.sh` dispatches the workflow for your current branch (`--ref` names another; it refuses to run `main` unless you say `--ref main`),
finds the run it created, and with `--watch` waits, prints the log of the failed jobs and exits 1 if the run failed. `--download DIR`
fetches the artifacts. `--target REF` runs the *tests* of another branch, tag or SHA with the scripts of your branch (useful to show that a
test fails on the old code). It needs the GitHub CLI (`gh`) logged in. `scripts/ci-run.sh --help` lists the suites.

| Suite | Workflow | What | Runs automatically |
|---|---|---|---|
| `ci` | `ci.yml` | the pull-request checks | every pull request and push to `main` |
| `smoke` | `e2e.yml` | GUI smoke test on macOS 15 and 26 | every pull request from this repository, push to `main` |
| `checklist` | `e2e.yml` | the full scripted QA checklist, macOS 15 and 26 | nightly 02:41 UTC |
| `focus` | `e2e.yml` | the focus regression checks (Quick Look, panel, click, idle) | on demand |
| `screenshots` | `e2e.yml` | preferences window, all pages, dark and light, as an artifact | push to `main` |
| `launcher-look` | `e2e.yml` | the launcher window, empty and with results, as an artifact | on demand |
| `x86-64` | `e2e.yml` | tests, native build and smoke test on the Intel image | push to `main` |
| `bundle` | `e2e.yml` | universal `.app` and DMG, verified | push to `main` |
| `ignored` | `stress.yml` | `#[ignore = "ci: ..."]` tests in release mode, full fuzz counts | nightly 03:41 UTC |
| `timing` | `stress.yml` | clock-dependent tests under CPU load, many runs, arm64 and Intel | nightly (short) |
| `perf-gui` | `stress.yml` | GUI start-up, hotkey latency, idle CPU and memory | nightly |
| `bench` | `stress.yml` | the benchmarks | nightly |
| `test-times` | `stress.yml` | the time of every test, alone | nightly |
| `soak` | `soak.yml` | the 2-hour soak | nightly 04:41 UTC |

Every workflow takes `workflow_dispatch` inputs `suite` and `ref` (soak: `soak_seconds` and `ref`), has `timeout-minutes` and a
`concurrency` group that cancels superseded runs of the same ref. [CI.md](CI.md) describes the jobs, the runners and their differences.

## Marking tests that are CI-only

### Rust

```rust
#[test]
#[ignore = "ci: waits for the OS file watcher, about 4 s"]
fn watcher_reports_config_and_workflow_edits_but_not_noise() { /* ... */ }
```

* The reason **starts with `ci:`** and says why the test cannot run on your machine. (`known: <finding>` is the other form: a test that
  demonstrates an open security finding and is un-ignored when it is fixed.) The policy check rejects any other `#[ignore]`.
* CI runs them with `cargo test --workspace -- --ignored` in **release mode**, one at a time (`scripts/ci/run-ignored.sh`, suite
  `ignored`). Absolute latency ceilings are `perf_*` tests that start with `crate::testperf::require_release()`; they only make sense there.
* Keep a **fast variant** locally when the test has regression value: put the body in a helper that takes the size and write two tests
  (`candidate_scoring_equals_string_scoring_on_random_input` runs 900 cases, `..._on_many_random_inputs` is `ci:` and runs 40,000).
* The fuzz suites (`crates/core/tests/fuzz.rs`) run a tenth of every case count (at least 8) on your machine and the full counts on CI
  (`ASHDASH_FUZZ_FULL=1`); `PROPTEST_CASES=N` overrides both.
* A test that is `ci:` but cannot work on a hosted runner at all goes in the `skip` list of `scripts/ci/run-ignored.sh`, with the reason.
* Do not write tests that compare a time with a constant to guard performance. Count work, compare scaling, or write a `perf_` test
  ([CI.md](CI.md#timing-sensitive-tests)).

### Shell

A script that is not called by `scripts/check.sh` is not judged by the policy check; keep it that way. End-to-end scripts live in
`tests/e2e/`, refuse to run outside a GitHub-hosted runner (`tests/e2e/ci/common.sh`, `scripts/qa/require-ci.sh`) and are only called by workflows.
If a script that the gate calls must keep one of the forbidden things, use the documented opt-out below.

## How to add an end-to-end test

1. Write the check in `tests/e2e/qa/checklist.sh` as a row in the section it belongs to (`result <id> PASS|FAIL "evidence"`), or a new
   section function `sNN` added to the default list in `main` at the bottom (and to the usage comment at the top); use `start_app`, `ask`, `look`, `win_tool` and the other helpers of
   `tests/e2e/qa/lib.sh`. For a standalone scenario add a script next to `smoke.sh` or in `tests/e2e/focus/`.
2. The script must source the CI guard (`tests/e2e/ci/common.sh` does `ci_require_hosted`; scripts started through a wrapper inherit it).
   Never write a TCC row for the app itself: `tests/e2e/guest/tcc-grant.sh` grants only the test tools.
3. A row that cannot work on a hosted runner is listed with its reason in `tests/e2e/ci/blocked.tsv` (it shows up as BLOCKED, not hidden).
4. Run it: `git push -u origin HEAD`, then `scripts/ci-run.sh checklist --sections s31 --watch --download out` (or `smoke`). Read the job
   summary and the artifacts; the screenshots are kept for failures only (7 days).
5. If the new check should gate pull requests, add it to the smoke test (`tests/e2e/smoke.sh`); otherwise it runs nightly in the checklist.
6. Add a row to the checklist report names in `tests/e2e/qa/report.py` and to [QA.md](QA.md) when it is a user-visible behaviour.

## What the policy check rejects

`scripts/policy-check.sh` (also a job `policy` of `ci.yml`, and a stage of `scripts/check.sh`) is a static, text-only check of the code that runs
on your machine: every Rust test that is not ignored (and the helpers it calls) in `crates/*/tests/`, in `#[cfg(test)]` modules and in
test files such as `tests.rs`, and `scripts/check.sh` plus every shell script it runs. It is a backstop with fixed thresholds, not a proof:
reviewers still read the tests.

| Rule | Rejected | Limit |
|---|---|---|
| `gui` | `Command::new("osascript" / "screencapture" / "open" / "tart" / "qlmanage" / "killall" ...)`, pasteboard and AppKit/winit calls, synthetic key events, `engine::notify`, `RealOs` running one of those tools; in shell: the same commands | none |
| `sleep` | `thread::sleep` with literal durations that add up | 200 ms per Rust function, 2 s per script |
| `loop` | `for` or iterator over a literal range, `vec![x; N]`, `.take(N)`, `"x".repeat(N)` | 1000 iterations, 5000 items, 64 MiB of bytes, 1,000,000 characters |
| `network` | `TcpStream::connect`, `UdpSocket`, HTTP client crates, `curl`, `wget`, `ssh`, `gh`, `git push/fetch/clone`, `cargo install`, `brew` | none (unix sockets and loopback listeners are fine) |
| `clock` | `assert!` comparing `elapsed()` (or a latency variable) with a `Duration` | a bound of 5 s or more is a hang guard and passes |
| `heavy` | shell only: `cargo bench`, release builds, `--ignored`, packaging, `tests/e2e/`, `scripts/qa/`, `scripts/ci/`, `cargo update` | none |
| `ignore` | an `#[ignore]` without `"ci: ..."` or `"known: ..."` | cannot be opted out |

**Opt-out**, for the rare justified case, is a comment with a reason of at least ten characters, so that a reviewer can judge it:

```rust
// policy-allow: loop -- the 5,000-hit cap is under test and 6,000 hits is the size that exceeds it; it takes milliseconds
let noise = (0..6000).map(...);

// policy-allow-fn: clock -- a hang guard: the case takes microseconds, an unbounded run would never return   (whole function that follows)
```

```sh
# policy-allow: network -- talks to a server on 127.0.0.1 that this test starts itself        (this line and the next)
# policy-allow-file: gui -- this file only holds the text of scripts that the checker reads    (the whole file; the scripts it names are not followed)
```

`scripts/policy-check.sh --list-allowed` prints every opt-out with its reason. The tests of the check, including the negative cases that prove
it fails for each forbidden thing, are `tests/policy/policy-check.test.sh`.

## How tests are classified

1. `scripts/ci/test-times.sh` runs every test alone (one thread) and prints the slowest; CI runs it as the suite `test-times` (debug, and release with
   the ignored tests), and the table is in the job summary and the artifact. It uses `RUSTC_BOOTSTRAP=1` for the test binaries only, because
   `--report-time` is not stable. It is not run on a developer's machine.
2. A test over the thresholds above is marked `#[ignore = "ci: ..."]` (with a fast variant if it has regression value) or reworked to count
   work instead of time. The full list of what moved is in the pull request that introduced this policy.
3. Re-run `test-times` when the gate gets slow; `scripts/check.sh`'s stage table says which stage grew.

## What reviewers check

* A new test is fast (no sleeps over 200 ms, no loops of 1000, no wall-clock assertion), deterministic and touches no real GUI, pasteboard or network. If it cannot be, it is `#[ignore = "ci: ..."]` with a fast variant, or an e2e check.
* The `policy` and `check` jobs are green; an opt-out has a reason a stranger can judge, and there are few of them.
* A new script that drives the screen or the keyboard lives in `tests/e2e/`, refuses to run outside CI, and is run by a workflow, not by `scripts/check.sh`.
* Nothing was added to `scripts/check.sh` that pushes it toward the budget; its stage table is in the job summary of `check`.
* A change to UI behaviour has an e2e row or a smoke step, and the pull request says which suite was run (`scripts/ci-run.sh smoke` green, or the run URL).
* No local VM, no `tart`, no screen recording on a laptop.

## Security of the workflows

The repository is public. The workflows use `pull_request` only (never `pull_request_target`), a read-only token, no secrets and actions
pinned by commit SHA. **Pull requests from forks run the cheap jobs of `ci.yml` only** (`check`, `deny`, `commit-lint`, `hooks`, `policy`,
`packaging`): the end-to-end and stress workflows drive a GUI session as an administrator, may write privacy-permission rows and run for
hours, and must not run code from a stranger; a maintainer pushes the branch to this repository to run them. Artifacts are kept for failures
or because they are the result (screenshots, bundle, performance numbers) and expire after 7 days.

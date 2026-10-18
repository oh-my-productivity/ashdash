#!/usr/bin/env bash
# Runs a CI suite for your branch, from your terminal: dispatches the workflow, optionally watches it, prints the logs of the failed jobs
# and downloads the artifacts. This is how end-to-end, GUI, stress and performance work is done: it never runs on a developer's machine
# (docs/TESTING.md).
#
#   scripts/ci-run.sh <suite> [--ref REF] [--target REF] [--watch] [--download DIR] [--dry-run] [-f name=value ...]
#
# Suites (the workflow in brackets):
#   ci            the pull-request checks, on demand (ci.yml)
#   smoke         the GUI smoke test on macOS 15 and 26 (e2e.yml)
#   checklist     the full scripted QA checklist on macOS 15 and 26 (e2e.yml; --sections "s31 s32" runs some of it)
#   screenshots   the preferences window, every page, dark and light, as an artifact (e2e.yml)
#   x86-64        tests, native build and GUI smoke test on the Intel image (e2e.yml)
#   bundle        the universal .app and the DMG, verified (e2e.yml)
#   e2e           smoke, checklist, screenshots, x86-64 and bundle together (e2e.yml)
#   ignored       the #[ignore = "ci: ..."] Rust tests and the full fuzz counts, in release mode (stress.yml)
#   timing        the formerly clock-dependent tests, many times, under CPU load (stress.yml; --runs N, --slow-runs N)
#   perf-gui      start-up, hotkey latency, idle CPU and memory of the real app (stress.yml)
#   bench         the criterion benchmarks (stress.yml)
#   test-times    the time of every test, alone (stress.yml)
#   stress        ignored, timing, perf-gui, bench and test-times together (stress.yml)
#   soak          the 2-hour soak (soak.yml; --soak-seconds N shortens it)
#
# Options:
#   --ref REF         the branch to run; default: the branch you are on. It must exist on origin (push it first: git push -u origin HEAD).
#                     Refused on main unless you say so with --ref main.
#   --target REF      test the code of another branch, tag or full SHA with the workflow and scripts of --ref (stress/e2e suites)
#   --watch           wait for the run, print the log of the failed jobs, exit with the run's result (1 when it failed)
#   --download DIR    after the run finished, download its artifacts into DIR (implies --watch)
#   --dry-run         print what would be done and do nothing (needs neither gh nor the network)
#   --sections "..."  checklist sections          --soak-seconds N   soak length          --runs N, --slow-runs N   timing runs
#   -f name=value     any other workflow input (repeatable)
#
# Needs the GitHub CLI (gh), logged in, and your branch pushed. The workflow files must exist on the default branch for GitHub to
# accept a dispatch of a branch: a brand-new workflow file is first tested by a pull request or a temporary push trigger.
# policy-allow-file: network, heavy -- this is the tool that talks to GitHub and names the CI suites; the local gate only runs it with --dry-run
set -euo pipefail
cd "$(dirname "$0")/.."

die() { echo "ci-run: $*" >&2; exit 2; }
usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; }

suite="" ref="" target="" watch=0 download="" dry=0
declare -a inputs=()
while [ $# -gt 0 ]; do
  case "$1" in
    -h | --help) usage; exit 0 ;;
    --ref) [ $# -ge 2 ] || die "--ref needs a value"; ref=$2; shift 2 ;;
    --target) [ $# -ge 2 ] || die "--target needs a value"; target=$2; shift 2 ;;
    --watch) watch=1; shift ;;
    --download) [ $# -ge 2 ] || die "--download needs a directory"; download=$2; watch=1; shift 2 ;;
    --dry-run) dry=1; shift ;;
    --sections) [ $# -ge 2 ] || die "--sections needs a value"; inputs+=("sections=$2"); shift 2 ;;
    --soak-seconds) [ $# -ge 2 ] || die "--soak-seconds needs a value"; inputs+=("soak_seconds=$2"); shift 2 ;;
    --runs) [ $# -ge 2 ] || die "--runs needs a value"; inputs+=("runs=$2"); shift 2 ;;
    --slow-runs) [ $# -ge 2 ] || die "--slow-runs needs a value"; inputs+=("slow_runs=$2"); shift 2 ;;
    -f | --input) [ $# -ge 2 ] || die "$1 needs name=value"; case $2 in *=*) ;; *) die "$1 needs name=value, got '$2'" ;; esac; inputs+=("$2"); shift 2 ;;
    -*) die "unknown option $1 (see --help)" ;;
    *) [ -z "$suite" ] || die "one suite at a time (got '$suite' and '$1')"; suite=$1; shift ;;
  esac
done
[ -n "$suite" ] || { usage >&2; exit 2; }

# suite -> workflow file and its `suite` input ("" = the workflow has none)
case $suite in
  ci) workflow=ci.yml; wf_suite="" ;;
  smoke | checklist | screenshots | x86-64 | bundle) workflow=e2e.yml; wf_suite=$suite ;;
  e2e) workflow=e2e.yml; wf_suite=all ;;
  ignored | timing | perf-gui | bench | test-times) workflow=stress.yml; wf_suite=$suite ;;
  stress) workflow=stress.yml; wf_suite=all ;;
  soak) workflow=soak.yml; wf_suite="" ;;
  *) die "unknown suite '$suite'. Suites: ci smoke checklist screenshots x86-64 bundle e2e ignored timing perf-gui bench test-times stress soak" ;;
esac

# the branch to run
if [ -z "$ref" ]; then
  ref=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)
  [ -n "$ref" ] || die "HEAD is detached: say which branch to run with --ref BRANCH"
  case $ref in
    main | master) die "you are on $ref: a suite is run for a branch. Create one and push it, or run $ref on purpose with --ref $ref" ;;
  esac
fi
case $ref in
  *[!A-Za-z0-9._/-]* | "" | -*) die "refusing the branch name '$ref'" ;;
esac

# workflow inputs: only the ones the workflow declares are accepted by GitHub, so the optional ones are added only when asked for
args=(workflow run "$workflow" --ref "$ref")
[ -z "$wf_suite" ] || args+=(-f "suite=$wf_suite")
[ -z "$target" ] || args+=(-f "ref=$target")
for kv in ${inputs[@]+"${inputs[@]}"}; do args+=(-f "$kv"); done

if [ "$dry" = 1 ]; then
  echo "ci-run: dry run, nothing is dispatched"
  echo "ci-run: would check that origin has the branch: git ls-remote --exit-code --heads origin $ref"
  echo "gh ${args[*]}"
  if [ "$watch" = 1 ]; then echo "ci-run: then: gh run list --workflow $workflow --branch $ref --event workflow_dispatch  (newest run), gh run watch <id> --exit-status, gh run view <id> --log-failed"; fi
  if [ -n "$download" ]; then echo "ci-run: then: gh run download <id> --dir $download"; fi
  exit 0
fi

command -v gh >/dev/null 2>&1 || die "the GitHub CLI is needed: https://cli.github.com/ (brew install gh), then gh auth login"
gh auth status >/dev/null 2>&1 || die "gh is not logged in: gh auth login"
remote_sha=$(git ls-remote --heads origin "refs/heads/$ref" 2>/dev/null | awk '{ print $1 }' | head -n 1)
[ -n "$remote_sha" ] || die "the branch '$ref' is not on origin: git push -u origin $ref"
if [ "$ref" = "$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)" ] && [ "$(git rev-parse HEAD)" != "$remote_sha" ]; then
  echo "ci-run: warning: origin/$ref is at ${remote_sha:0:9} but your HEAD is $(git rev-parse --short=9 HEAD): the run tests what is pushed" >&2
fi

before=$(date -u +%Y-%m-%dT%H:%M:%SZ)
gh "${args[@]}"
echo "ci-run: dispatched $workflow${wf_suite:+ (suite $wf_suite)} on $ref"

# find the run that the dispatch created (GitHub returns no id): the newest dispatch of this workflow on this branch since 'before'
run_id=""
for _ in $(seq 1 30); do
  run_id=$(gh run list --workflow "$workflow" --branch "$ref" --event workflow_dispatch --limit 10 \
    --json databaseId,createdAt --jq "[.[] | select(.createdAt >= \"$before\")] | sort_by(.createdAt) | last | .databaseId // empty" 2>/dev/null || true)
  [ -n "$run_id" ] && break
  sleep 2
done
[ -n "$run_id" ] || die "the run did not show up; look at: gh run list --workflow $workflow --branch $ref"
url=$(gh run view "$run_id" --json url --jq .url)
echo "ci-run: $url"
[ "$watch" = 1 ] || { echo "ci-run: follow it with: gh run watch $run_id --exit-status"; exit 0; }

rc=0
gh run watch "$run_id" --exit-status --interval 20 >/dev/null 2>&1 || rc=$?
conclusion=$(gh run view "$run_id" --json conclusion --jq .conclusion)
echo "ci-run: finished: $conclusion"
gh run view "$run_id" --json jobs --jq '.jobs[] | "  \(.conclusion // .status)\t\(.name)"'
if [ "$conclusion" != success ]; then
  echo "ci-run: ---- log of the failed jobs (last 200 lines) ----"
  gh run view "$run_id" --log-failed 2>&1 | tail -n 200 || true
fi
if [ -n "$download" ]; then
  mkdir -p "$download"
  gh run download "$run_id" --dir "$download" 2>&1 || echo "ci-run: no artifacts to download"
  echo "ci-run: artifacts are in $download"
fi
[ "$conclusion" = success ] || rc=1
exit "$rc"

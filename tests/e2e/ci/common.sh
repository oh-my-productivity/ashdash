#!/usr/bin/env bash
# Shared by run-smoke.sh, run-checklist.sh and run-soak.sh. Sourced, never executed.
# shellcheck shell=bash
#
# These wrappers run the end-to-end scripts directly on a GitHub-hosted macOS runner: a hosted runner is an Apple virtual machine with a
# logged-in GUI session, SIP off and passwordless sudo (docs/CI.md). Because the scripts take over the keyboard, the screen and the global
# hotkey, they refuse to run anywhere else: end-to-end tests run on CI only, never on a developer's Mac (docs/TESTING.md).

CI_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
export CI_ROOT

ci_die() { printf 'ci: %s\n' "$*" >&2; exit 1; }

ci_require_hosted() {
  [ "$(uname -s)" = Darwin ] || ci_die "refusing: macOS only"
  if [ "${GITHUB_ACTIONS:-}" != true ] || [ "${RUNNER_ENVIRONMENT:-}" != github-hosted ]; then
    ci_die "refusing: this drives the real keyboard, screen and global hotkey and may write TCC rows; it runs only on a GitHub-hosted runner (GITHUB_ACTIONS=true, RUNNER_ENVIRONMENT=github-hosted). Run it on CI: scripts/ci-run.sh (docs/TESTING.md)."
  fi
  if [ "$(id -un)" != runner ] || [ "$(stat -f %Su /dev/console)" != runner ]; then
    ci_die "refusing: expected the logged-in 'runner' account (user $(id -un), console $(stat -f %Su /dev/console))"
  fi
}

# Where the logs, results and (on failure only) screenshots go: uploaded by the workflow when the job fails.
ci_artifacts() { # ci_artifacts <name> -> prints and creates the directory
  local d=${CI_ARTIFACTS:-${RUNNER_TEMP:-/tmp}/ashdash-ci}/$1
  mkdir -p "$d"
  printf '%s\n' "$d"
}

# Markdown for the job summary page (no-op outside Actions).
ci_summary() { if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then cat >>"$GITHUB_STEP_SUMMARY"; else cat >/dev/null; fi; }

# Screenshots are evidence for failures only: a green run keeps none of them.
ci_drop_screenshots() { find "$1" -type f \( -name '*.png' -o -name '*.jpg' \) -delete 2>/dev/null || true; }

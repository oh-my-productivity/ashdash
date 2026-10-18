# shellcheck shell=bash
# Sourced first by the QA scripts that drive the real desktop (windows, keyboard, hotkey, screenshots): they run on CI only.
# Testing policy (docs/TESTING.md): a developer's machine runs unit and integration tests; everything that takes over the screen or the
# keyboard, or measures performance, runs on a GitHub-hosted runner. Run the suite with scripts/ci-run.sh instead.
if [ "${GITHUB_ACTIONS:-}" != true ] || [ "${RUNNER_ENVIRONMENT:-}" != github-hosted ]; then
  echo "$(basename "$0"): refusing to run here: this drives the real keyboard, screen and global hotkey, and end-to-end tests run on CI only." >&2
  echo "$(basename "$0"): push your branch and run the suite there: scripts/ci-run.sh <suite> --watch  (docs/TESTING.md)" >&2
  exit 2
fi

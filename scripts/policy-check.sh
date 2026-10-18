#!/usr/bin/env bash
# The static half of the testing policy (docs/TESTING.md): fails when a test that runs on a developer's machine does what only CI may do
# (real GUI/OS calls, long sleeps, big loops, the network, wall-clock assertions) or when a script of the local gate does.
#   scripts/policy-check.sh [--root DIR] [--list-allowed]
# Pure text analysis, about a second. Runs in scripts/check.sh and in the `policy` job of ci.yml. The rules are in scripts/policy_check.py.
set -euo pipefail
exec python3 "$(dirname "$0")/policy_check.py" "$@"

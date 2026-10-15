#!/usr/bin/env bash
# The single gate every work package must pass before review.
set -euo pipefail
cd "$(dirname "$0")/.."
echo "== fmt";    cargo fmt --all -- --check
echo "== clippy"; cargo clippy --workspace --all-targets -- -D warnings
echo "== test";   cargo test --workspace
echo "check.sh: OK"

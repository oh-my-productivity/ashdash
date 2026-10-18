#!/usr/bin/env bash
# Print the value of one constant of crates/core/src/format_ids.rs (the identifiers of the
# workflow package and appearance file formats), so shell scripts need not spell them out.
#   scripts/format-id.sh WORKFLOW_EXT          -> the workflow package extension (no dot)
#   scripts/format-id.sh INPUT_KEYWORD         -> an object type
set -euo pipefail
src="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/crates/core/src/format_ids.rs"
[ $# -eq 1 ] || { echo "usage: format-id.sh NAME" >&2; exit 2; }
value="$(sed -n "s/^[[:space:]]*pub const $1: &str = \"\\([^\"]*\\)\";.*/\\1/p" "$src" | head -n 1)"
[ -n "$value" ] || { echo "format_ids.rs has no constant $1" >&2; exit 1; }
printf '%s' "$value"

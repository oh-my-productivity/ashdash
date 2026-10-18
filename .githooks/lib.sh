# shellcheck shell=bash
# Shared by the hooks in this folder (sourced, not executed). Written for bash 3.2, the macOS system bash.
# The caller sets HOOK (its name) before sourcing.

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

say() { printf '%s: %s\n' "$HOOK" "$*"; }
warn() { printf '%s: warning: %s\n' "$HOOK" "$*" >&2; }
have() { command -v "$1" >/dev/null 2>&1; }

# The words the repository must not contain are derived from the format-id module, never written out here.
# Prints the lower-case name (empty when the module is missing); the retired working name is that plus "o".
forbidden_name() {
  local v
  v=$("$ROOT/scripts/format-id.sh" ENV_PREFIX 2>/dev/null) || return 0
  v=$(printf '%s' "$v" | tr 'A-Z' 'a-z')
  printf '%s' "${v%_}"
}

# The files names.rs lets mention the name (it lists them in ALLOWED_FILES); read from there so the two cannot drift.
names_allowed_files() {
  local src="$ROOT/crates/core/tests/names.rs"
  [ -r "$src" ] || return 0
  sed -n '/ALLOWED_FILES/,/\];/p' "$src" | grep -o '"[^"]*"' | tr -d '"'
}

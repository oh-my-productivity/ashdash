#!/usr/bin/env bash
# Runs shellcheck (warnings and errors) on every shell script of the repository and actionlint on the workflows.
#   tests/lab/ci/lint.sh            both tools must be on PATH (CI installs pinned releases; locally: brew install shellcheck actionlint)
# Scripts are found by their shebang, not by extension, so extensionless helpers (lab-up, gui-exec, ...) are included.
set -uo pipefail
cd "$(dirname "$0")/../../.." || exit 1
for tool in shellcheck actionlint; do
  command -v "$tool" >/dev/null 2>&1 || { echo "lint: $tool not found on PATH" >&2; exit 2; }
done
shellcheck --version | sed -n 2p
actionlint -version | head -1

rc=0
files=()
while IFS= read -r f; do
  case "$f" in docs/* | crates/*) continue ;; esac
  [ -f "$f" ] && [ ! -L "$f" ] || continue
  if head -c 64 "$f" | head -1 | grep -Eq '^#!.*(/|env )(ba)?sh'; then files+=("$f"); fi
done < <(git ls-files)
echo "== shellcheck: ${#files[@]} scripts"
shellcheck -x -S warning "${files[@]}" || rc=1
echo "== actionlint: $(ls .github/workflows/*.yml | wc -l | tr -d ' ') workflows"
actionlint || rc=1
[ "$rc" = 0 ] && echo "lint: OK"
exit "$rc"

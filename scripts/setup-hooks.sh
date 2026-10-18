#!/usr/bin/env bash
# Installs the repository's git hooks for THIS clone: `git config --local core.hooksPath .githooks`. Never touches the global or system
# configuration, copies nothing into .git/hooks, and is safe to run again.
#   scripts/setup-hooks.sh              install (idempotent), then report which optional tools are present
#   scripts/setup-hooks.sh --status     show what is installed
#   scripts/setup-hooks.sh --uninstall  remove the setting again (git then uses .git/hooks, as before)
#   scripts/setup-hooks.sh --force      replace a different core.hooksPath that is already set in this clone
# What the hooks check: CONTRIBUTING.md, "Commit messages and pull requests". Hooks can be skipped (git commit --no-verify); CI repeats them.
set -eu
cd "$(dirname "${BASH_SOURCE[0]}")/.."
HOOKS=.githooks

action=install force=0
for a in "$@"; do
  case $a in
    --uninstall) action=uninstall ;;
    --status) action=status ;;
    --force) force=1 ;;
    -h | --help) sed -n '2,/^# What the hooks/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "setup-hooks: unknown argument: $a (try --help)" >&2; exit 2 ;;
  esac
done

git rev-parse --git-dir >/dev/null 2>&1 || { echo "setup-hooks: this is not a git checkout" >&2; exit 1; }
[ -d "$HOOKS" ] || { echo "setup-hooks: $HOOKS not found; run this from a checkout that has it" >&2; exit 1; }
current=$(git config --local --get core.hooksPath || true)

optional_tools() {
  local t
  for t in shellcheck actionlint gitleaks; do
    if command -v "$t" >/dev/null 2>&1; then echo "  $t: found (used by the hooks)"; else echo "  $t: not installed (that check is skipped; brew install $t)"; fi
  done
}

case $action in
  status)
    if [ "$current" = "$HOOKS" ]; then echo "setup-hooks: installed (core.hooksPath = $HOOKS)"; else echo "setup-hooks: not installed (core.hooksPath = ${current:-unset})"; fi
    optional_tools ;;
  uninstall)
    if [ "$current" = "$HOOKS" ]; then
      git config --local --unset core.hooksPath
      echo "setup-hooks: removed core.hooksPath from this clone; git uses .git/hooks again"
    elif [ -z "$current" ]; then
      echo "setup-hooks: nothing to do, the hooks were not installed"
    else
      echo "setup-hooks: core.hooksPath is \"$current\", not $HOOKS; left alone" >&2
    fi ;;
  install)
    if [ -n "$current" ] && [ "$current" != "$HOOKS" ] && [ "$force" = 0 ]; then
      echo "setup-hooks: this clone already uses core.hooksPath = \"$current\"; run with --force to replace it" >&2
      exit 1
    fi
    for h in commit-msg pre-commit pre-push; do
      [ -x "$HOOKS/$h" ] || chmod +x "$HOOKS/$h"
    done
    if [ "$current" = "$HOOKS" ]; then echo "setup-hooks: already installed (core.hooksPath = $HOOKS)"; else
      git config --local core.hooksPath "$HOOKS"
      echo "setup-hooks: installed for this clone (core.hooksPath = $HOOKS)"
    fi
    echo "  commit-msg: Conventional Commits   pre-commit: fmt, secrets, whitespace, sizes, names   pre-push: clippy, tests, commit messages"
    echo "  skip once: git commit --no-verify / git push --no-verify; skip the cargo steps of pre-push: SKIP_PREPUSH=1 git push"
    optional_tools
    email=$(git config user.email || true)
    case $email in
      *@users.noreply.github.com) ;;
      *) echo "  warning: user.email is \"${email:-unset}\": the e-mail of your commits is public. Use your GitHub noreply address:" \
           "git config user.email <id>+<name>@users.noreply.github.com" ;;
    esac
    echo "  optional commit template: git config commit.template .gitmessage" ;;
esac

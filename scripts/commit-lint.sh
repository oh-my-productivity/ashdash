#!/usr/bin/env bash
# Validates Conventional Commits 1.0.0 messages: `type(scope)!: subject`, optional body, optional footers.
# No dependencies beyond bash 3.2 (the macOS system bash), git (range mode only) and the standard tools (awk, tr, wc).
#
#   scripts/commit-lint.sh --file .git/COMMIT_EDITMSG    a message file (what the commit-msg hook passes; `-` reads standard input)
#   scripts/commit-lint.sh --title "feat(gui): add x"    a pull request title (one line, strict: no Merge/Revert/fixup! forms)
#   scripts/commit-lint.sh --message "$(git log -1 --format=%B)"   a message in a string
#   scripts/commit-lint.sh --range origin/main..HEAD      every non-merge commit of a git range
#
# Options: --forbid-names   also refuse the two old product names (derived from crates/core/src/format_ids.rs, never spelled here)
#          --max-length N   longest header, in characters (default 72, or $COMMIT_LINT_MAX_HEADER)
#          --quiet          print nothing on success
#
# Rules (the owner's decisions are in CONTRIBUTING.md, "Commit messages and pull requests"):
#   * header = type[(scope)][!]: subject; types feat fix perf refactor docs test build ci chore style revert (lower case);
#   * scope is optional and free text without spaces, parentheses or colons (gui, core, deps-dev, gui/prefs, a.b, ...);
#   * subject: one space after the colon, does not start with an upper-case ASCII letter, no trailing period;
#   * the header is at most 72 characters (characters, not bytes: an accented letter counts once);
#   * a body or footers are separated from the header by a blank line; `BREAKING CHANGE: <text>` needs text.
# Git-made messages are accepted in message and range mode (not as a PR title): `Merge ...`, `Revert "..."`, `fixup! ...`,
# `squash! ...`, `amend! ...`. Lines starting with `#` are dropped in --file mode, as git does.
# Exit status: 0 valid, 1 at least one message is invalid, 2 usage or git error.
set -u
export LC_ALL=C   # byte-wise matching: ASCII classes mean ASCII, and the character count below does not depend on the user's locale

TYPES="feat fix perf refactor docs test build ci chore style revert"
MAX_HEADER=${COMMIT_LINT_MAX_HEADER:-72}
QUIET=0
FORBID_NAMES=0
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

usage() {
  sed -n '2,/^# Exit status/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
}

die() { echo "commit-lint: $*" >&2; exit 2; }

# ---- helpers ---------------------------------------------------------------------------------------------------------------

# normalize <strip-comments 0|1>: stdin -> stdout. Removes CRs, trailing blanks, leading/trailing empty lines; with 1 also the
# `#` lines and everything from git's scissors line on (what `git commit` does with the default cleanup in an editor).
normalize() {
  tr -d '\r' | awk -v strip="$1" '
    strip == 1 && /^# -+ >8 -+$/ { exit }
    strip == 1 && /^#/ { next }
    { sub(/[ \t]+$/, ""); lines[++n] = $0 }
    END {
      s = 1; while (s <= n && lines[s] == "") s++
      e = n; while (e >= s && lines[e] == "") e--
      for (i = s; i <= e; i++) print lines[i]
    }'
}

# char_count <string>: characters of a UTF-8 string (bytes that are not continuation bytes), independent of the locale.
char_count() {
  local s=$1 n
  if [ "${#s}" -le "$MAX_HEADER" ]; then echo "${#s}"; return; fi   # LC_ALL=C: ${#s} counts bytes, an upper bound of the characters
  n=$(printf '%s' "$s" | tr -d '\200-\277' | wc -c)
  echo $((n + 0))
}

lower() { printf '%s' "$1" | tr 'A-Z' 'a-z'; }

is_type() { local t; for t in $TYPES; do [ "$t" = "$1" ] && return 0; done; return 1; }

# The retired working name contains the other one, so one needle covers both. Printed lower case, empty when unknown.
forbidden_needle() {
  local v
  v=$("$ROOT/scripts/format-id.sh" ENV_PREFIX 2>/dev/null) || return 0
  v=$(lower "$v")
  printf '%s' "${v%_}"
}

ERRS=""
NOTES=""
add_err() { ERRS="$ERRS  - $1"$'\n'; }
add_note() { NOTES="$NOTES  note: $1"$'\n'; }

EXAMPLES='Valid examples:
  feat(gui): add a dark appearance to the preferences window
  fix: keep the window on the screen that holds the cursor
  refactor(core)!: rename the snippets store

Types: feat fix perf refactor docs test build ci chore style revert. Scope optional. A breaking change is marked with "!" or a
"BREAKING CHANGE: <what breaks>" footer. Subject: imperative, starts in lower case, no trailing period, header at most 72 characters.
Details: CONTRIBUTING.md, "Commit messages and pull requests".'

# check_message <message> <mode commit|title>: fills ERRS and NOTES for one message.
check_message() {
  local breaking_re='^BREAKING[ -]CHANGE: [^[:space:]]'
  local msg=$1 mode=$2 header rest line2 prefix after p type scope has_scope=0 subject first n
  ERRS=""; NOTES=""
  if [ -z "$msg" ]; then add_err "the message is empty"; return; fi
  header=${msg%%$'\n'*}
  if [ "$mode" = title ] && [ "$header" != "$msg" ]; then
    add_err "a pull request title is one line; move the rest to the description"
    msg=$header
  fi

  if [ "$mode" = commit ]; then   # messages that git writes itself
    case $header in
      "Merge "*) add_note "merge commit: accepted here, but pull requests are squash-merged, so merge commits do not reach main"; return ;;
      'Revert "'*'"') add_note "git revert message: accepted (the squash-merged pull request title decides what main gets; use 'revert: ...' there)"; return ;;
      "fixup! "?* | "squash! "?* | "amend! "?*) add_note "autosquash commit: accepted; it disappears when the pull request is squash-merged"; return ;;
    esac
  fi

  if [[ $header != *:* ]]; then
    add_err "the header has no ':' - it must look like  type(scope): subject"
  else
    prefix=${header%%:*}
    after=${header#*:}
    p=$prefix
    if [[ $p == *'!' ]]; then p=${p%!}; fi
    type=$p
    if [[ $p == *')' ]]; then
      if [[ $p == *'('* ]]; then
        type=${p%%(*}
        scope=${p#*(}; scope=${scope%)}
        has_scope=1
      else
        add_err "unbalanced ')' before the colon"
      fi
    elif [[ $p == *'('* ]]; then
      add_err "the scope is not closed: write type(scope): subject"
      type=${p%%(*}
    fi

    if [ "$has_scope" = 1 ]; then
      if [ -z "$scope" ]; then
        add_err "the scope is empty: remove the parentheses or name an area, for example (gui)"
      else
        case $scope in
          *[[:space:]\(\)\!]*) add_err "the scope \"$scope\" must not contain spaces, parentheses or '!'" ;;
        esac
      fi
    fi

    if ! is_type "$type"; then
      if [ -z "$type" ]; then
        add_err "the type is missing before the scope or colon"
      elif [[ $type == *'!'* ]]; then
        add_err "the '!' goes after the scope, right before the colon: type(scope)!: subject"
      elif [[ $type == *[[:space:]]* ]]; then
        add_err "no space is allowed before the colon or inside the type"
      elif is_type "$(lower "$type")"; then
        add_err "the type must be lower case: \"$(lower "$type")\" instead of \"$type\""
      else
        add_err "unknown type \"$type\" (allowed: $TYPES)"
      fi
    fi

    if [ -z "$after" ] || [ "$after" = " " ]; then
      add_err "the subject is empty"
    elif [[ $after != ' '* ]]; then
      add_err "put one space after the colon"
    else
      subject=${after# }
      if [[ $subject == ' '* ]]; then
        add_err "only one space after the colon"
        subject=${subject#"${subject%%[! ]*}"}
      fi
      first=${subject:0:1}
      if [[ $first == [A-Z] ]]; then
        add_err "the subject starts in lower case (\"${subject%% *}\" -> \"$(lower "${subject%% *}")\"; or reword if it is an acronym)"
      fi
      if [[ $subject == *. ]]; then add_err "the subject must not end with a period"; fi
      case $(lower "${subject%% *}") in
        added | adds | fixed | fixes | updated | updates | removed | removes | changed | changes | adding | fixing | updating | removing | changing)
          add_note "the subject is imperative (\"add\", not \"added\" or \"adds\")" ;;
      esac
    fi
  fi

  n=$(char_count "$header")
  if [ "$n" -gt "$MAX_HEADER" ]; then add_err "the header is $n characters; the limit is $MAX_HEADER"; fi

  if [ "$header" != "$msg" ]; then
    rest=${msg#*$'\n'}
    line2=${rest%%$'\n'*}
    if [ -n "$line2" ]; then add_err "leave an empty line between the header and the body (line 2 is not empty)"; fi
    while IFS= read -r line; do
      case $line in
        "BREAKING CHANGE:"* | "BREAKING-CHANGE:"*)
          if [[ ! $line =~ $breaking_re ]]; then
            add_err "\"BREAKING CHANGE:\" needs a description after the colon and one space: BREAKING CHANGE: <what breaks>"
          fi ;;
      esac
    done <<EOF
$rest
EOF
  fi

  if [ "$FORBID_NAMES" = 1 ]; then
    local needle
    needle=$(forbidden_needle)
    if [ -n "$needle" ] && [[ $(lower "$msg") == *"$needle"* ]]; then
      add_err "the message contains a name that must not appear in the repository (CONTRIBUTING.md, \"Names\"); reword it"
    fi
  fi
}

# report <label> <message>: prints the problems of the current ERRS/NOTES; returns 1 when there are problems.
report() {
  local label=$1 header=$2
  if [ -n "$ERRS" ]; then
    {
      echo "commit-lint: INVALID $label"
      echo "  header: $header"
      printf '%s' "$ERRS"
      [ -n "$NOTES" ] && printf '%s' "$NOTES"
      echo
      echo "$EXAMPLES"
    } >&2
    return 1
  fi
  [ -n "$NOTES" ] && { echo "commit-lint: $label" >&2; printf '%s' "$NOTES" >&2; }
  return 0
}

ok() { [ "$QUIET" = 1 ] || echo "commit-lint: ok - $*"; }

# ---- modes -----------------------------------------------------------------------------------------------------------------

lint_string() { # lint_string <label> <message> <mode>
  check_message "$2" "$3"
  local header=${2%%$'\n'*}
  report "$1" "$header" || return 1
  ok "$1"
}

lint_range() {
  local range=$1 sha total=0 bad=0 merges msg word words
  [ -n "$range" ] || die "the range is empty"
  # The range is handed to git rev-list as words (so "topic --not --remotes=origin" works); only --not and --remotes are options.
  read -r -a words <<<"$range"
  for word in "${words[@]}"; do
    case $word in --not | --remotes | --remotes=*) ;; -*) die "invalid range word: $word" ;; esac
  done
  git rev-parse --git-dir >/dev/null 2>&1 || die "not inside a git repository"
  git rev-list "${words[@]}" -- >/dev/null 2>&1 || die "git cannot resolve the range \"$range\" (is the base branch fetched? use fetch-depth: 0 in CI)"
  merges=$(git rev-list --merges "${words[@]}" -- | wc -l | tr -d ' ')
  while IFS= read -r sha; do
    [ -n "$sha" ] || continue
    total=$((total + 1))
    msg=$(git log -1 --format=%B "$sha" | normalize 0)
    check_message "$msg" commit
    report "commit ${sha:0:10}" "${msg%%$'\n'*}" || bad=$((bad + 1))
  done < <(git rev-list --reverse --no-merges "${words[@]}" --)
  [ "$merges" -gt 0 ] && echo "commit-lint: skipped $merges merge commit(s); pull requests are squash-merged" >&2
  if [ "$bad" -gt 0 ]; then
    echo "commit-lint: $bad of $total commit(s) in $range are invalid. Fix them with  git rebase -i  (reword), or squash the branch." >&2
    return 1
  fi
  ok "$total commit(s) in $range"
}

# ---- arguments -------------------------------------------------------------------------------------------------------------

mode="" arg=""
set_mode() { [ -z "$mode" ] || die "give exactly one of --file, --title, --message, --range"; mode=$1; arg=${2-}; }
while [ $# -gt 0 ]; do
  case $1 in
    -h | --help) usage; exit 0 ;;
    --file | --title | --message | --range)
      [ $# -ge 2 ] || die "$1 needs a value"
      set_mode "${1#--}" "$2"; shift 2 ;;
    --forbid-names) FORBID_NAMES=1; shift ;;
    --quiet) QUIET=1; shift ;;
    --max-length)
      [ $# -ge 2 ] || die "--max-length needs a value"
      MAX_HEADER=$2; shift 2 ;;
    *) usage; die "unknown argument: $1" ;;
  esac
done
case $MAX_HEADER in '' | *[!0-9]*) die "the maximum length must be a number, got: $MAX_HEADER" ;; esac
[ -n "$mode" ] || { usage; exit 2; }

case $mode in
  file)
    if [ "$arg" = "-" ]; then text=$(normalize 1); else
      [ -r "$arg" ] || die "cannot read the message file: $arg"
      text=$(normalize 1 <"$arg")
    fi
    lint_string "message" "$text" commit || exit 1 ;;
  message)
    text=$(printf '%s\n' "$arg" | normalize 0)
    lint_string "message" "$text" commit || exit 1 ;;
  title)
    text=$(printf '%s\n' "$arg" | normalize 0)
    lint_string "pull request title" "$text" title || exit 1 ;;
  range)
    lint_range "$arg" || exit 1 ;;
esac
exit 0

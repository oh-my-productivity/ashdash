#!/usr/bin/env bash
# Tests of scripts/commit-lint.sh: message cases in all four modes. Needs bash, git, awk. Runs in about 3 s.
set -u
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
LINT="$REPO/scripts/commit-lint.sh"
TMP=$(mktemp -d "${TMPDIR:-/tmp}/commit-lint-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

# ok <description> <message> / bad <description> <message> [expected text in the output]
ok() {
  local out rc
  out=$("$LINT" --message "$2" 2>&1); rc=$?
  expect_rc 0 "valid: $1" "$rc" "$out"
}
bad() {
  local out rc
  out=$("$LINT" --message "$2" 2>&1); rc=$?
  expect_rc 1 "invalid: $1" "$rc" "$out"
  [ -z "${3-}" ] || expect_contains "message for: $1" "$out" "$3"
}

# ---- headers: valid ----------------------------------------------------------------------------------------------------------
ok "every type"                      'feat: a'
for t in feat fix perf refactor docs test build ci chore style revert; do ok "type $t" "$t: do the thing"; done
ok "scope"                           'feat(gui): add a dark appearance'
ok "breaking mark"                   'feat!: drop the old config file'
ok "scope and breaking mark"         'refactor(core)!: rename the snippets store'
ok "scope with a dash"               'build(deps-dev): bump tempfile'
ok "scope with a slash"              'fix(gui/prefs): keep the tab on reload'
ok "scope with a dot"                'fix(core.engine): rank exact matches first'
ok "scope with a comma"              'fix(core,gui): share the key table'
ok "scope with an underscore"        'chore(format_ids): regenerate'
ok "scope with digits"               'ci(macos-15): pin the runner'
ok "scope with a plus"               'docs(c++): note the bridge'
ok "scope with unicode"              'docs(日本語): add a note'
ok "dependabot title"                'build(deps): bump the minor-and-patch group with 3 updates'
ok "subject starting with a digit"   'fix: 2 windows opened on startup'
ok "subject starting with a symbol"  'fix: `ashdash query` crashed on empty input'
ok "subject starting with a quote"   'docs: "quoted" start is fine'
ok "colon inside the subject"        'fix(gui): handle error: connection refused'
ok "parentheses inside the subject"  'fix: handle (nested) parentheses (really)'
ok "exclamation in the subject"      'feat: add a bang!'
ok "unicode subject"                 'fix(gui): zeige Umlaute richtig an: äöüß'
ok "cjk subject"                     'fix: 修复中文输入法的候选窗口'
ok "emoji subject"                   'docs: add a rocket 🚀 to the readme'
ok "trailing spaces are ignored"     'fix: a   '
ok "ellipsis ending is not a period" 'fix: wait for the index…'

# ---- headers: invalid --------------------------------------------------------------------------------------------------------
bad "empty message"                  '' "empty"
bad "plain sentence"                 'Update the readme' "no ':'"
bad "old style capital subject"      'Fix the crash' "no ':'"
bad "unknown type"                   'feature: add x' 'unknown type "feature"'
bad "upper-case type"                'Feat: add x' "lower case"
bad "all caps type"                  'FIX: add x' "lower case"
bad "type with a space"              'feat fix: x' "no space"
bad "space before the colon"         'feat : x' "no space"
bad "missing space after the colon"  'feat:x' "one space after the colon"
bad "two spaces after the colon"     'feat:  x' "only one space"
bad "empty subject"                  'feat: ' "subject is empty"
bad "empty subject without space"    'feat:' "subject is empty"
bad "empty scope"                    'feat(): x' "scope is empty"
bad "scope with a space"             'feat(a b): x' "must not contain spaces"
bad "nested parentheses in a scope"  'feat(a(b)): x' "must not contain"
bad "unclosed scope"                 'feat(gui: x' "not closed"
bad "bang before the scope"          'feat!(gui): x' "goes after the scope"
bad "bang twice"                     'feat!!: x' "goes after the scope"
bad "upper-case subject"             'feat: Add x' "lower case"
bad "upper-case acronym subject"     'fix: CJK font fallback' "lower case"
bad "trailing period"                'feat: add x.' "period"
bad "missing type"                   ': add x' "type is missing"
bad "missing type before a scope"    '(gui): add x' "type is missing"
bad "only a type"                    'feat' "no ':'"
bad "merge-style text as a title"    'WIP: stuff' 'unknown type "WIP"'

# ---- length: 72 characters, counted as characters ----------------------------------------------------------------------------
pad() { printf '%*s' "$1" '' | tr ' ' 'x'; }   # pad <n>: n letters
ok  "header of exactly 72 characters"       "fix: $(pad 67)"
bad "header of 73 characters"               "fix: $(pad 68)" "73 characters"
ok  "72 characters with multi-byte letters" "fix: $(printf 'é%.0s' $(seq 1 67))"
bad "73 characters with multi-byte letters" "fix: $(printf 'é%.0s' $(seq 1 68))" "73 characters"
ok  "72 characters of cjk (216 bytes)"      "fix: $(printf '文%.0s' $(seq 1 67))"
ok  "long body lines are fine"              "fix: a

$(pad 200)"

dep='build(deps): bump the minor-and-patch group across 1 directory with 12 updates'
bad "a long Dependabot group title is over the default limit" "$dep" "78 characters"
out=$(COMMIT_LINT_MAX_HEADER=120 "$LINT" --title "$dep" 2>&1); rc=$?; expect_rc 0 "...and fine with the Dependabot limit CI uses" "$rc" "$out"

# ---- bodies and footers ------------------------------------------------------------------------------------------------------
ok  "body"                           $'fix: a\n\nWhy it broke.'
ok  "multi-paragraph body"           $'feat: a\n\nFirst paragraph\nstill the first.\n\nSecond paragraph.\n\n- a list\n- of things'
ok  "body and footers"               $'fix: a\n\nWhy.\n\nRefs: #12\nReviewed-by: Someone <someone@users.noreply.github.com>'
ok  "footer with a hash reference"   $'fix: a\n\nCloses #12'
ok  "breaking change footer"         $'feat: a\n\nBREAKING CHANGE: the config file moved'
ok  "breaking change dash footer"    $'feat: a\n\nBREAKING-CHANGE: the config file moved'
ok  "breaking change in the body of a ! commit" $'feat(core)!: a\n\nBREAKING CHANGE: x\nmore of the description'
ok  "windows line endings"           $'fix: a\r\n\r\nbody\r\n'
ok  "leading blank lines are cut"    $'\n\nfix: a\n'
ok  "prose that starts with the words" $'fix: a\n\nBREAKING CHANGES are listed in the notes.'
bad "no blank line before the body"  $'fix: a\nbody' "empty line"
bad "empty breaking change footer"   $'feat: a\n\nBREAKING CHANGE:' "needs a description"
bad "breaking change without space"  $'feat: a\n\nBREAKING CHANGE:x' "needs a description"
bad "bad header with a good body"    $'Fix stuff\n\nBREAKING CHANGE: x' "no ':'"

# ---- git-made messages: fine in a commit, not as a title ----------------------------------------------------------------------
ok  "merge commit"                   "Merge branch 'main' into feature/x"
ok  "merge of a pull request"        'Merge pull request #12 from user/branch'
ok  "git revert message"             $'Revert "feat: add x"\n\nThis reverts commit 0123456789abcdef.'
ok  "fixup commit"                   'fixup! feat(gui): add x'
ok  "squash commit"                  'squash! feat(gui): add x'
ok  "amend commit"                   'amend! feat(gui): add x'
bad "fixup without a target"         'fixup! ' "no ':'"
bad "Revert without quotes"          'Revert feat: add x' "no space"
bad "Merge without a space"          'Merged stuff' "no ':'"

# ---- imperative hint is only a note --------------------------------------------------------------------------------------------
out=$("$LINT" --message 'fix: added a check' 2>&1); rc=$?
expect_rc 0 "past tense is accepted" "$rc" "$out"
expect_contains "past tense gets a note" "$out" "imperative"

# ---- title mode ----------------------------------------------------------------------------------------------------------------
t_ok()  { local out rc; out=$("$LINT" --title "$2" 2>&1); rc=$?; expect_rc 0 "title valid: $1" "$rc" "$out"; }
t_bad() { local out rc; out=$("$LINT" --title "$2" 2>&1); rc=$?; expect_rc 1 "title invalid: $1" "$rc" "$out"; }
t_ok  "conventional title"         'feat(gui): add a dark appearance'
t_ok  "breaking title"             'fix(core)!: drop the v1 snippet format'
t_ok  "revert type"                'revert: feat(gui): add a dark appearance'
t_bad "merge text is not a title"  "Merge branch 'main' into x"
t_bad "git revert text"            'Revert "feat: add x"'
t_bad "fixup is not a title"       'fixup! feat: add x'
t_bad "sentence title"             'Add a dark appearance'
t_bad "multi-line title"           $'feat: a\n\nbody'
t_bad "title with a period"        'feat: add x.'

# ---- message files: what git hands to commit-msg --------------------------------------------------------------------------------
f="$TMP/msg"
printf 'feat(gui): add x\n\n# Please enter the commit message for your changes. Lines starting\n# with # will be ignored.\n#\n# On branch main\n' >"$f"
out=$("$LINT" --file "$f" 2>&1); rc=$?; expect_rc 0 "file: comment lines are dropped" "$rc" "$out"
printf '# only a comment\n' >"$f"
out=$("$LINT" --file "$f" 2>&1); rc=$?; expect_rc 1 "file: only comments is an empty message" "$rc" "$out"
printf 'feat: add x\n\n# ------------------------ >8 ------------------------\n# Do not modify or remove the line above.\ndiff --git a/x b/x\n+Bad Line.\n' >"$f"
out=$("$LINT" --file "$f" 2>&1); rc=$?; expect_rc 0 "file: nothing after the scissors line is read" "$rc" "$out"
printf 'Bad subject\n' >"$f"
out=$("$LINT" --file "$f" 2>&1); rc=$?; expect_rc 1 "file: invalid message" "$rc" "$out"
expect_contains "file: the error shows an example" "$out" "feat(gui): add a dark appearance"
out=$(printf 'fix: from stdin\n' | "$LINT" --file - 2>&1); rc=$?; expect_rc 0 "file: standard input" "$rc" "$out"
out=$("$LINT" --file "$TMP/does-not-exist" 2>&1); rc=$?; expect_rc 2 "file: unreadable file is a usage error" "$rc" "$out"

# ---- usage ---------------------------------------------------------------------------------------------------------------------
out=$("$LINT" 2>&1); rc=$?; expect_rc 2 "no mode" "$rc" "$out"
out=$("$LINT" --bogus 2>&1); rc=$?; expect_rc 2 "unknown option" "$rc" "$out"
out=$("$LINT" --title 'fix: a' --message 'fix: b' 2>&1); rc=$?; expect_rc 2 "two modes" "$rc" "$out"
out=$("$LINT" --max-length x --title 'fix: a' 2>&1); rc=$?; expect_rc 2 "non-numeric max length" "$rc" "$out"
out=$("$LINT" --help 2>&1); rc=$?; expect_rc 0 "help" "$rc" "$out"
out=$("$LINT" --quiet --title 'fix: a' 2>&1); rc=$?; expect_rc 0 "quiet success" "$rc" "$out"
[ -z "$out" ] && pass || fail "quiet prints nothing" "$out"

# ---- options -------------------------------------------------------------------------------------------------------------------
out=$("$LINT" --max-length 20 --title 'fix: a message of 31 chars' 2>&1); rc=$?; expect_rc 1 "--max-length lowers the limit" "$rc" "$out"
out=$("$LINT" --max-length 100 --title "fix: $(pad 90)" 2>&1); rc=$?; expect_rc 0 "--max-length raises the limit" "$rc" "$out"
out=$(COMMIT_LINT_MAX_HEADER=100 "$LINT" --title "fix: $(pad 90)" 2>&1); rc=$?; expect_rc 0 "COMMIT_LINT_MAX_HEADER" "$rc" "$out"

# --forbid-names: the words come from the format-id module, so they are not spelled in this file
name=$("$REPO/scripts/format-id.sh" ENV_PREFIX | tr 'A-Z' 'a-z'); name=${name%_}
out=$("$LINT" --forbid-names --message "docs: compare with $name" 2>&1); rc=$?; expect_rc 1 "names: the product name is refused" "$rc" "$out"
out=$("$LINT" --forbid-names --message "docs: compare with $(printf '%s' "$name" | tr 'a-z' 'A-Z')" 2>&1); rc=$?; expect_rc 1 "names: case-insensitive" "$rc" "$out"
out=$("$LINT" --forbid-names --message "docs: compare with ${name}o" 2>&1); rc=$?; expect_rc 1 "names: the retired working name is refused" "$rc" "$out"
out=$("$LINT" --forbid-names --message $'docs: fine\n\nbody mentions '"$name" 2>&1); rc=$?; expect_rc 1 "names: also in the body" "$rc" "$out"
expect_lacks "names: the message does not print the word" "$out" "$name"
out=$("$LINT" --forbid-names --message 'docs: nothing to see' 2>&1); rc=$?; expect_rc 0 "names: clean message" "$rc" "$out"
out=$("$LINT" --message "docs: compare with $name" 2>&1); rc=$?; expect_rc 0 "names: not checked without the option" "$rc" "$out"

# ---- range mode, on a synthetic repository ----------------------------------------------------------------------------------------
R="$TMP/repo"
git init -q "$R" && cd "$R" || exit 1
git config user.email t@users.noreply.github.com; git config user.name T; git config commit.gpgsign false
commit() { git commit -q --allow-empty -m "$1" || exit 1; }
commit 'Initial commit (old style, before the convention)'
git branch -M main
git checkout -q -b topic
commit 'feat(gui): add x'
commit $'fix: y\n\nbody text'
out=$("$LINT" --range main..topic 2>&1); rc=$?; expect_rc 0 "range: valid commits" "$rc" "$out"
expect_contains "range: counts the commits" "$out" "2 commit(s)"
commit 'Bad message one'
commit 'Bad Message: two'
out=$("$LINT" --range main..topic 2>&1); rc=$?; expect_rc 1 "range: invalid commits" "$rc" "$out"
expect_contains "range: names the bad commit" "$out" "Bad message one"
expect_contains "range: summary" "$out" "2 of 4 commit(s)"
expect_lacks "range: does not blame the valid ones" "$out" "header: feat(gui): add x"
out=$("$LINT" --range main..main 2>&1); rc=$?; expect_rc 0 "range: empty range" "$rc" "$out"
out=$("$LINT" --range nosuchbranch..topic 2>&1); rc=$?; expect_rc 2 "range: unknown ref is a usage error" "$rc" "$out"
out=$("$LINT" --range '--all' 2>&1); rc=$?; expect_rc 2 "range: refuses option-like values" "$rc" "$out"
git reset -q --hard HEAD~2
git checkout -q main; commit 'chore: advance main'
git checkout -q topic
git merge -q --no-ff -m "Merge branch 'main' into topic" main
out=$("$LINT" --range main..topic 2>&1); rc=$?; expect_rc 0 "range: merge commits are skipped" "$rc" "$out"
git commit -q --allow-empty -m 'fixup! feat(gui): add x'
out=$("$LINT" --range main..topic 2>&1); rc=$?; expect_rc 0 "range: fixup commits are accepted" "$rc" "$out"
cd "$REPO" || exit 1
out=$(cd "$TMP" && "$LINT" --range main..topic 2>&1); rc=$?; expect_rc 2 "range: outside a repository" "$rc" "$out"

summary "commit-lint"

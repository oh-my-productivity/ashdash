#!/usr/bin/env bash
# Tests of ci-plan.sh and verify-release-commit.sh (the two scripts the release workflow relies on).
set -euo pipefail
# shellcheck source=scripts/release/test/lib-testrepo.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib-testrepo.sh"
require_cliff_or_skip
export PREPARE_OFFLINE=1 CARGO_NET_OFFLINE=true

PLAN="$REL_DIR/ci-plan.sh"
VERIFY="$REL_DIR/verify-release-commit.sh"

# run_plan: environment INPUT_* from the caller; sets PLAN_STATUS, PLAN_OUT (GITHUB_OUTPUT) and PLAN_SUMMARY.
run_plan() {
  : >"$TEST_TMP/gh-out"
  : >"$TEST_TMP/gh-summary"
  PLAN_STATUS=0
  GITHUB_OUTPUT="$TEST_TMP/gh-out" GITHUB_STEP_SUMMARY="$TEST_TMP/gh-summary" "$PLAN" >/dev/null 2>"$TEST_TMP/plan.err" || PLAN_STATUS=$?
  PLAN_OUT="$(cat "$TEST_TMP/gh-out")"
  PLAN_SUMMARY="$(cat "$TEST_TMP/gh-summary")"
}

after_010() { new_repo 0.1.0 --lock; release 0.1.0; }

echo "== ci-plan.sh"
after_010; c "feat(gui): a thing (#9)"
INPUT_BUMP=auto INPUT_VERSION='' INPUT_PRERELEASE=false run_plan
expect_eq "plan status" "$PLAN_STATUS" "0"
expect_in "plan: release=true" "$PLAN_OUT" "release=true"
expect_in "plan: version" "$PLAN_OUT" "version=0.2.0"
expect_in "plan: tag" "$PLAN_OUT" "tag=v0.2.0"
expect_in "plan: not a pre-release" "$PLAN_OUT" "prerelease=false"
expect_in "plan: sha is HEAD" "$PLAN_OUT" "sha=$(git rev-parse HEAD)"
expect_in "plan: summary has the version" "$PLAN_SUMMARY" 'Next version: `0.2.0`'
expect_in "plan: summary lists the deciding commit" "$PLAN_SUMMARY" "feat(gui): a thing (#9)"

INPUT_BUMP=auto INPUT_VERSION='' INPUT_PRERELEASE=true run_plan
expect_in "plan: release candidate version" "$PLAN_OUT" "version=0.2.0-rc.1"
expect_in "plan: release candidate flag" "$PLAN_OUT" "prerelease=true"

INPUT_BUMP=patch INPUT_VERSION='' INPUT_PRERELEASE=false run_plan
expect_in "plan: manual bump" "$PLAN_OUT" "version=0.1.1"

INPUT_BUMP=auto INPUT_VERSION=0.7.0 INPUT_PRERELEASE=false run_plan
expect_in "plan: exact version" "$PLAN_OUT" "version=0.7.0"

INPUT_BUMP=auto INPUT_VERSION=0.7.0-rc.1 INPUT_PRERELEASE=true run_plan
expect_in "plan: exact candidate version wins over the checkbox" "$PLAN_OUT" "version=0.7.0-rc.1"

after_010; c "docs: only docs"
INPUT_BUMP=auto INPUT_VERSION='' INPUT_PRERELEASE=false run_plan
expect_eq "nothing to release is not an error" "$PLAN_STATUS" "0"
expect_in "nothing to release: release=false" "$PLAN_OUT" "release=false"
expect_not_in "nothing to release: no version output" "$PLAN_OUT" "version="
expect_in "nothing to release: summary" "$PLAN_SUMMARY" "Nothing to release"

# The inputs come from the workflow dispatch form; a hostile value must be rejected, never executed.
marker="$TEST_TMP/pwned"
INPUT_BUMP=auto INPUT_VERSION="1.2.3; touch $marker" INPUT_PRERELEASE=false run_plan
[ "$PLAN_STATUS" != 0 ] && [ ! -e "$marker" ] && pass "plan: a shell command in the version input is rejected, not run" || fail "plan: injection"
INPUT_BUMP='minor; touch $marker' INPUT_VERSION='' INPUT_PRERELEASE=false run_plan
[ "$PLAN_STATUS" != 0 ] && [ ! -e "$marker" ] && pass "plan: a shell command in the bump input is rejected, not run" || fail "plan: injection (bump)"
INPUT_BUMP=auto INPUT_VERSION='' INPUT_PRERELEASE=maybe run_plan
expect_eq "plan: bad boolean" "$([ "$PLAN_STATUS" != 0 ] && echo failed)" "failed"

after_010; c "fix: a"; echo "# edit" >>CHANGELOG.md
INPUT_BUMP=auto INPUT_VERSION='' INPUT_PRERELEASE=false run_plan
expect_eq "plan: a dirty tree fails" "$([ "$PLAN_STATUS" = 65 ] && echo yes)" "yes"
expect_in "plan: failure is in the summary" "$PLAN_SUMMARY" "The plan failed"

echo "== verify-release-commit.sh"
# make_release_commit VERSION: prepares and commits like the workflow does; sets BASE and COMMIT.
make_release_commit() {
  BASE="$(git rev-parse HEAD)"
  "$REL_DIR/prepare.sh" "$1" --date 2030-01-02 >/dev/null 2>&1
  git add Cargo.toml Cargo.lock CHANGELOG.md crates/app/Cargo.toml crates/core/Cargo.toml
  git commit -q -m "chore(release): v$1"
  COMMIT="$(git rev-parse HEAD)"
}
expect_verify() { # NAME STATUS NEEDLE ARGS...
  local name="$1" want="$2" needle="$3" status=0
  shift 3
  "$VERIFY" "$@" >"$TEST_TMP/v.out" 2>"$TEST_TMP/v.err" || status=$?
  if [ "$status" != "$want" ]; then
    fail "$name" "wanted status $want, got $status" "$(cat "$TEST_TMP/v.err")"
  elif [ -n "$needle" ] && ! grep -qiF -- "$needle" "$TEST_TMP/v.err" "$TEST_TMP/v.out"; then
    fail "$name" "output does not mention '$needle'" "$(cat "$TEST_TMP/v.err")"
  else
    pass "$name"
  fi
}

after_010; c "fix: a"; make_release_commit 0.1.1
expect_verify "a proper release commit" 0 "well formed" "$BASE" "$COMMIT" 0.1.1
expect_verify "the version may carry a leading v" 0 "well formed" "$BASE" "$COMMIT" v0.1.1
expect_verify "wrong version" 65 "unexpected subject" "$BASE" "$COMMIT" 0.1.2
expect_verify "wrong base" 65 "exactly one parent" "$(git rev-parse "$BASE~1")" "$COMMIT" 0.1.1
expect_verify "not a commit" 65 "not a commit" "$BASE" 0000000000000000000000000000000000000000 0.1.1
expect_verify "bad version argument" 64 "not a release version" "$BASE" "$COMMIT" nonsense
expect_verify "missing arguments" 64 "usage" "$BASE"

after_010; c "fix: a"; BASE="$(git rev-parse HEAD)"
"$REL_DIR/prepare.sh" 0.1.1 --date 2030-01-02 >/dev/null 2>&1
echo 'fn main() { println!("injected"); }' >crates/app/src/main.rs
git add -A; git commit -q -m "chore(release): v0.1.1"
expect_verify "a source file smuggled into the release commit" 65 "never touches" "$BASE" "$(git rev-parse HEAD)" 0.1.1

after_010; c "fix: a"; BASE="$(git rev-parse HEAD)"
"$REL_DIR/prepare.sh" 0.1.1 --date 2030-01-02 >/dev/null 2>&1
git add -A; git commit -q -m "chore: bump"
expect_verify "wrong subject" 65 "unexpected subject" "$BASE" "$(git rev-parse HEAD)" 0.1.1

after_010; c "fix: a"; BASE="$(git rev-parse HEAD)"
"$REL_DIR/prepare.sh" 0.1.1 --date 2030-01-02 >/dev/null 2>&1
sed -i.bak 's/^name = "ashdash-core"/name = "ashdash-core-evil"/' Cargo.lock && rm Cargo.lock.bak
git add -A; git commit -q -m "chore(release): v0.1.1"
expect_verify "Cargo.lock changed beyond versions" 65 "more than version lines" "$BASE" "$(git rev-parse HEAD)" 0.1.1

after_010; c "fix: a"; BASE="$(git rev-parse HEAD)"
"$REL_DIR/prepare.sh" 0.1.1 --date 2030-01-02 >/dev/null 2>&1
sed -i.bak 's/^version = "0.1.1"/version = "0.1.9"/' crates/core/Cargo.toml && rm crates/core/Cargo.toml.bak
git add -A; git commit -q -m "chore(release): v0.1.1"
expect_verify "a crate at the wrong version" 65 "not 0.1.1" "$BASE" "$(git rev-parse HEAD)" 0.1.1

after_010; c "fix: a"; BASE="$(git rev-parse HEAD)"
git commit -q --allow-empty -m "chore(release): v0.1.1"
expect_verify "an empty commit" 65 "changes nothing" "$BASE" "$(git rev-parse HEAD)" 0.1.1

after_010; c "fix: a"; BASE="$(git rev-parse HEAD)"
"$REL_DIR/prepare.sh" 0.1.1 --date 2030-01-02 >/dev/null 2>&1
git add -A; git commit -q -m "chore(release): v0.1.1"; good="$(git rev-parse HEAD)"
git checkout -q "$BASE"; git checkout -q -b other; c "chore: another commit"
git merge -q --no-ff -m "chore(release): v0.1.1" "$good" 2>/dev/null || true
expect_verify "a merge commit" 65 "exactly one parent" "$BASE" "$(git rev-parse HEAD)" 0.1.1

echo "== first release: only the changelog changes"
new_repo 0.1.0 --lock; BASE="$(git rev-parse HEAD)"
"$REL_DIR/prepare.sh" 0.1.0 --date 2030-01-02 >/dev/null 2>&1
git add -A; git commit -q -m "chore(release): v0.1.0"
expect_verify "first release commit" 0 "well formed" "$BASE" "$(git rev-parse HEAD)" 0.1.0

echo "== preview.sh"
PREVIEW="$REL_DIR/preview.sh"
after_010; c "fix(core): a bug (#4)"
printf 'feat(gui): shiny new tab (#5)\n\nSome words.\n\nBREAKING CHANGE: the tab replaces the old one\n' >"$TEST_TMP/msg.txt"
before="$(git rev-parse HEAD)"
out="$("$PREVIEW" --message-file "$TEST_TMP/msg.txt" 2>"$TEST_TMP/pv.err")" || fail "preview runs" "$(cat "$TEST_TMP/pv.err")"
expect_in "preview: version without the pull request" "$out" "Without this pull request the next release would be **0.1.1**"
expect_in "preview: version with the pull request" "$out" "With it the next release would be **0.2.0**"
expect_in "preview: shows the header" "$out" "feat(gui): shiny new tab (#5)"
expect_in "preview: shows the notes" "$out" "### Breaking changes"
expect_in "preview: shows the diff stat" "$out" "CHANGELOG.md"
expect_eq "preview leaves the repository alone" "$(git rev-parse HEAD)$(git status --porcelain)" "$before"
printf 'docs: explain things\n' >"$TEST_TMP/msg2.txt"
out="$("$PREVIEW" --message-file "$TEST_TMP/msg2.txt" 2>/dev/null)"
expect_in "preview: a docs title does not change the version" "$out" "still **0.1.1**"
after_010
printf 'docs: explain things\n' >"$TEST_TMP/msg2.txt"
out="$("$PREVIEW" --message-file "$TEST_TMP/msg2.txt" 2>/dev/null)"
expect_in "preview: nothing to release before and after" "$out" "still nothing to release"
printf 'Fix the thing\n' >"$TEST_TMP/msg3.txt"
out="$("$PREVIEW" --message-file "$TEST_TMP/msg3.txt" 2>/dev/null)"
expect_in "preview: a title that is not conventional releases nothing" "$out" "still nothing to release"
out="$("$PREVIEW" 2>/dev/null)"
expect_in "preview without a pull request" "$out" "Nothing to release"
c "feat: x"
out="$("$PREVIEW" 2>/dev/null)"
expect_in "preview of main" "$out" "### Why 0.2.0"

finish "ci script tests"

#!/usr/bin/env bash
# Tests of next-version.sh on synthetic repositories. Needs the pinned git-cliff and jq (see lib-testrepo.sh).
set -euo pipefail
# shellcheck source=scripts/release/test/lib-testrepo.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib-testrepo.sh"
require_cliff_or_skip
command -v jq >/dev/null || { echo "jq is required" >&2; exit 1; }

# A repository whose last release is v0.1.0 (the usual starting point).
after_010() { new_repo 0.1.0; release 0.1.0; }

echo "== first release (no tag yet)"
new_repo 0.1.0
expect_version "first release is the version in Cargo.toml" 0.1.0
expect_in "first release says why" "$NV_ERR" "first release"
c "feat: something"
expect_version "first release ignores the commits" 0.1.0
expect_version "first release with --bump minor" 0.2.0 --bump minor
expect_version "first release as a release candidate" 0.1.0-rc.1 --prerelease rc
new_repo 0.4.0
expect_version "first release takes whatever Cargo.toml says" 0.4.0
new_repo 0.1.0-rc.2
git tag -a v0.1.0-rc.1 -m rc1
git tag -a v0.1.0-rc.2 -m rc2
c "docs: after the candidate"
expect_version "first stable release after release candidates" 0.1.0

echo "== which commits release (major version 0)"
after_010; c "docs: explain the hotkey"; c "docs(readme): typo"
expect_status "only docs: nothing to release" 10 "nothing to release"
after_010; c "chore: bump"; c "ci: pin"; c "test: more"; c "style: format"; c "build(deps): bump x"; c "refactor: tidy"
expect_status "chore ci test style build refactor: nothing to release" 10 "nothing to release"
after_010; c "fix: crash on empty query"
expect_version "fix only -> patch" 0.1.1
after_010; c "perf(core): faster index"
expect_version "perf only -> patch" 0.1.1
after_010; c "feat: new provider"
expect_version "feat -> minor" 0.2.0
after_010; c "feat(gui)!: new window API"
expect_version "feat! -> minor while major is 0" 0.2.0
after_010; c "fix!: drop a flag"
expect_version "fix! -> minor while major is 0" 0.2.0
after_010; c "docs: rewrite" "Details.

BREAKING CHANGE: config.json version 1 is no longer read"
expect_version "BREAKING CHANGE footer on a docs commit -> minor" 0.2.0
after_010; c "refactor: rename (#31)" "## Summary

- renames the config key
- updates the docs

## Checklist

- [x] tests
- [x] docs

BREAKING CHANGE: the key is called differently now"
expect_version "BREAKING CHANGE footer below a markdown pull request description -> minor" 0.2.0
after_010; c "fix: a small fix (#32)" "## Summary

- one line

Mentions BREAKING CHANGE: only in prose, in the middle of the text.

## Checklist

- [x] tests"
expect_version "prose that mentions BREAKING CHANGE in the middle is not a footer -> patch" 0.1.1
after_010; c "refactor: rename" "BREAKING-CHANGE: the old name is gone"
expect_version "BREAKING-CHANGE footer spelling -> minor" 0.2.0
after_010; c "docs: a"; c "fix: b"; c "feat: c"; c "chore: d"
expect_version "mixed fix feat docs chore -> minor" 0.2.0
after_010; c "docs: a"; c "fix(core,gui): b"; c "chore(deps): d"
expect_version "scope with a comma -> patch" 0.1.1
after_010; c "feat(gui/prefs): tabs (#42)"
expect_version "scope with a slash and a PR number -> minor" 0.2.0
after_010; c "revert: undo the window API"
expect_status "revert alone: nothing to release" 10 "nothing to release"
after_010; c 'Revert "feat: something"' 'This reverts commit abc.'
expect_status "git-generated Revert message is not conventional: nothing to release" 10 "nothing to release"
after_010; c "Fix the crash"; c "Update docs"; c "WIP"
expect_status "non-conventional commits never release" 10 "nothing to release"
after_010; c "Merge pull request #1 from x/y"
expect_status "a merge commit message never releases" 10 "nothing to release"
after_010; c "chore(release): v0.1.1"
expect_status "the release commit itself never releases" 10 "nothing to release"
after_010; c "fix: real fix"; c "chore(release): v0.1.1 (failed run, no tag)"
expect_version "a stray release commit does not hide a fix" 0.1.1

echo "== major version 1 and later"
new_repo 1.0.0; release 1.0.0; c "fix: x"
expect_version "1.0.0 + fix" 1.0.1
new_repo 1.0.0; release 1.0.0; c "feat: x"
expect_version "1.0.0 + feat" 1.1.0
new_repo 1.0.0; release 1.0.0; c "feat!: x"
expect_version "1.0.0 + feat!" 2.0.0
new_repo 1.4.2; release 1.4.2; c "docs: x" "BREAKING CHANGE: y"
expect_version "1.4.2 + breaking footer" 2.0.0

echo "== overrides"
new_repo 0.3.0; release 0.3.0; c "fix: x"
expect_version "--bump major from 0.3.0 is the 1.0.0 decision" 1.0.0 --bump major
expect_in "--bump major explains the 1.0.0 decision" "$NV_ERR" "1.0.0 decision"
expect_version "--bump minor" 0.4.0 --bump minor
expect_version "--bump patch" 0.3.1 --bump patch
expect_version "--bump auto is the default" 0.3.1 --bump auto
expect_version "--version exact" 0.9.0 --version 0.9.0
expect_version "--version accepts a leading v" 1.0.0 --version v1.0.0
expect_version "--version with a release candidate suffix" 0.4.0-rc.1 --version 0.4.0-rc.1
expect_status "--version that already exists" 65 "already exists" --version 0.3.0
expect_status "--version lower than the latest tag" 65 "not higher" --version 0.2.9
expect_status "--version that is not a version" 64 "not a release version" --version 1.2
expect_status "--version with a leading zero" 64 "not a release version" --version 01.2.3
expect_status "--version with an unknown suffix" 64 "not a release version" --version 1.2.3-beta
expect_status "--bump and --version together" 64 "not both" --bump minor --version 0.5.0
expect_status "--bump with a bad value" 64 "--bump must be" --bump huge
expect_status "--prerelease with a bad value" 64 "--prerelease must be" --prerelease nightly
expect_status "--prerelease with --version" 64 "--prerelease cannot be combined" --prerelease rc --version 0.5.0
expect_status "unknown argument" 64 "unknown argument" --frobnicate
new_repo 0.3.0; release 0.3.0; c "docs: only docs"
expect_version "--bump patch releases even when only docs changed" 0.3.1 --bump patch
new_repo 0.3.0; release 0.3.0; c "feat: x"
expect_version "--bump patch with a feat still works" 0.3.1 --bump patch
expect_in "--bump patch with a feat warns" "$NV_ERR" "ask for a minor bump"
new_repo 0.3.0; release 0.3.0
expect_status "--bump on a commit that is already released" 10 "already released" --bump patch

echo "== release candidates"
after_010; c "feat: a"
expect_version "first candidate" 0.2.0-rc.1 --prerelease rc
expect_version "a beta" 0.2.0-beta.1 --prerelease beta
release 0.2.0-rc.1
c "fix: b"
expect_version "second candidate of the same version" 0.2.0-rc.2 --prerelease rc
expect_version "the final release after a candidate" 0.2.0
release 0.2.0-rc.2
c "feat!: c"
expect_version "final version after two candidates" 0.2.0
expect_status "--version of an existing candidate" 65 "already exists" --version 0.2.0-rc.2
expect_version "--version for the next candidate" 0.2.0-rc.3 --version 0.2.0-rc.3
expect_status "--version below the latest candidate" 65 "not higher" --version 0.2.0-beta.1
after_010; c "fix: a"; release 0.1.1-rc.1; c "docs: after the candidate"
expect_version "stable tags decide the base, candidates only the Cargo check" 0.1.1

echo "== promoting a candidate"
after_010; c "feat: a"; release 0.2.0-rc.1
expect_version "the commit of a candidate can become the final release (computed)" 0.2.0
expect_version "the commit of a candidate can become the final release (exact)" 0.2.0 --version 0.2.0
expect_version "a second candidate on the same commit" 0.2.0-rc.2 --prerelease rc
expect_status "but the candidate itself cannot be released twice" 65 "already exists" --version 0.2.0-rc.1
after_010; c "fix: a"; release 0.1.1-rc.1
expect_status "a candidate whose commits do not release anything on their own" 65 "already exists" --version 0.1.1-rc.1

echo "== state of the repository"
after_010; c "fix: a"; echo "# change" >>crates/app/Cargo.toml
expect_status "dirty tracked file is refused" 65 "uncommitted changes"
expect_version "--allow-dirty accepts it" 0.1.1 --allow-dirty
after_010; c "fix: a"; echo "stray" >untracked.txt
expect_version "untracked files are fine" 0.1.1
after_010; c "fix: a"; set_version 0.1.5; git commit -qam "docs: hand edit"
expect_status "tag/Cargo mismatch" 65 "tag/Cargo mismatch"
after_010; c "fix: a"; sed -i.bak 's/^version = "0.1.0"/version = "0.1.1"/' crates/core/Cargo.toml; rm crates/core/Cargo.toml.bak; git commit -qam "docs: partial edit"
expect_status "app and core versions differ" 65 "different versions"
new_repo 0.1.0; release 0.1.0; c "fix: a"
src="$REPO"; shallow="$TEST_TMP/shallow"; git clone -q --depth 1 "file://$src" "$shallow"; cd "$shallow"
expect_status "shallow clone is refused" 65 "shallow"
cd "$src"
after_010; git checkout -q -b side; c "feat: only on a side branch"; release 0.9.0; git checkout -q main; c "fix: a"
expect_version "tags that HEAD cannot reach are ignored" 0.1.1
after_010; git tag nightly; git tag v1; git tag -a vNext -m x; c "fix: a"
expect_version "tags that are not release tags are ignored" 0.1.1
after_010; c "fix: a"; release 0.1.1
expect_status "HEAD already released" 10 "already released"
cd "$TEST_TMP" && mkdir -p empty && cd empty
expect_status "outside a git repository" 65 "not inside a git repository"
after_010; c "fix: a"
GIT_CLIFF=/nonexistent/git-cliff expect_status "missing git-cliff" 69 "git-cliff not found"

echo "== explanation and reason file"
after_010; c "docs: a"; c "fix(core): the bug (#7)"; c "feat(gui): the feature (#8)"
run_nv --reason-file "$TEST_TMP/reason.md"
expect_eq "stdout is exactly the version" "$NV_OUT" "0.2.0"
expect_in "stderr names the previous release" "$NV_ERR" "previous stable release: v0.1.0"
expect_in "stderr names the deciding feat" "$NV_ERR" "feat(gui): the feature (#8)"
expect_not_in "stderr leaves out the docs commit" "$NV_ERR" "docs: a"
reason="$(cat "$TEST_TMP/reason.md")"
expect_in "reason file has the version" "$reason" "Next version: **0.2.0**"
expect_in "reason file lists the commit" "$reason" "feat(gui): the feature (#8)"
after_010; c "docs: a"
run_nv --reason-file "$TEST_TMP/reason2.md"
expect_in "reason file for nothing to release" "$(cat "$TEST_TMP/reason2.md")" "Nothing to release since"

finish "next-version tests"

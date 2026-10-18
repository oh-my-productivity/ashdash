#!/usr/bin/env bash
# Tests of prepare.sh on synthetic repositories. Needs the pinned git-cliff, jq and cargo (offline, no dependencies).
set -euo pipefail
# shellcheck source=scripts/release/test/lib-testrepo.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib-testrepo.sh"
require_cliff_or_skip
command -v jq >/dev/null || { echo "jq is required" >&2; exit 1; }
command -v cargo >/dev/null || { echo "cargo is required" >&2; exit 1; }

export PREPARE_OFFLINE=1 CARGO_NET_OFFLINE=true
export CLIFF_REPO_URL="https://example.invalid/me/repo"

PREPARE="$REL_DIR/prepare.sh"
# run_prepare ARGS...: sets P_STATUS and P_ERR.
run_prepare() {
  P_STATUS=0
  "$PREPARE" "$@" >"$TEST_TMP/p.out" 2>"$TEST_TMP/p.err" || P_STATUS=$?
  P_ERR="$(cat "$TEST_TMP/p.err")"
}
expect_prepare_ok() {
  local name="$1"
  shift
  run_prepare "$@"
  if [ "$P_STATUS" = 0 ]; then pass "$name"; else fail "$name" "status $P_STATUS" "$P_ERR"; fi
}
expect_prepare_fails() { # NAME STATUS NEEDLE ARGS...
  local name="$1" want="$2" needle="$3"
  shift 3
  run_prepare "$@"
  if [ "$P_STATUS" != "$want" ]; then
    fail "$name" "wanted status $want, got $P_STATUS" "$P_ERR"
  elif ! grep -qiF -- "$needle" <<<"$P_ERR"; then
    fail "$name" "stderr does not mention '$needle'" "$P_ERR"
  else
    pass "$name"
  fi
}

# A repository that has released 0.1.0 and has a lock file.
after_010_lock() {
  new_repo 0.1.0 --lock
  release 0.1.0
}

echo "== versions in the manifests and the lock file"
after_010_lock; c "fix(core): handle an empty query (#12)"
expect_prepare_ok "prepare 0.1.1" 0.1.1 --date 2030-01-02
expect_eq "app version" "$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml)" "0.1.1"
expect_eq "core version" "$(sed -n 's/^version = "\(.*\)"/\1/p' crates/core/Cargo.toml)" "0.1.1"
expect_in "path dependency version follows" "$(cat crates/app/Cargo.toml)" 'ashdash-core = { version = "0.1.1", path = "../core" }'
expect_eq "Cargo.lock: exactly the two version lines changed" "$(git diff --numstat Cargo.lock | cut -f1,2)" "$(printf '2\t2')"
expect_eq "Cargo.lock: only version lines" "$(git diff -U0 Cargo.lock | grep -E '^[+-][^+-]' | grep -vc '^[+-]version = ')" "0"
if cargo metadata --locked --offline --format-version 1 >/dev/null 2>&1; then pass "cargo accepts the lock file (--locked)"; else fail "cargo accepts the lock file (--locked)"; fi
expect_eq "only the expected files changed" "$(git status --porcelain | sed 's/^...//' | LC_ALL=C sort | tr '\n' ' ')" "CHANGELOG.md Cargo.lock crates/app/Cargo.toml crates/core/Cargo.toml "
expect_eq "dist/ is not tracked" "$(git status --porcelain | grep -c dist || true)" "0"

echo "== the changelog section"
after_010_lock
c "docs: explain the hotkey"
c "fix(core): handle an empty query (#12)"
c "feat(gui)!: new window API (#13)"
c "perf(core): faster index"
c "feat(prefs): a tab for themes (#14)"
c "refactor: tidy the engine"
c "chore(deps): bump x"
c "chore(release): v0.1.1"
c "fix(core): squash title with a description (#15)" "The description GitHub puts below the title.

It even mentions (#99) in a later line."
c "Merge pull request #5 from a/b"
c "ci: pin"
c "docs: rewrite" "BREAKING CHANGE: the old config is not read any more, thanks @someone"
expect_prepare_ok "prepare a mixed release" 0.2.0 --date 2030-01-02
log="$(cat CHANGELOG.md)"
expect_in "heading with the date" "$log" "## [0.2.0] - 2030-01-02"
for g in "### Breaking changes" "### Added" "### Fixed" "### Performance" "### Changed" "### Documentation" "### Other"; do
  expect_in "section $g" "$log" "$g"
done
expect_eq "sections come in the fixed order" "$(grep '^### ' CHANGELOG.md | tr '\n' ',')" "### Breaking changes,### Added,### Fixed,### Performance,### Changed,### Documentation,### Other,"
expect_in "a pull request number becomes a link" "$log" "([#12](https://example.invalid/me/repo/pull/12))"
expect_in "a pull request number is found when a description follows" "$log" "([#15](https://example.invalid/me/repo/pull/15))"
expect_in "a commit becomes a link" "$log" "https://example.invalid/me/repo/commit/"
expect_in "scope in bold" "$log" "**gui**: new window API"
expect_in "breaking marker" "$log" "**BREAKING** **gui**: new window API"
expect_in "breaking description" "$log" "- the old config is not read any more, thanks"
expect_in "mentions are defanged" "$log" '`@someone`'
expect_not_in "the release commit is left out" "$log" "chore(release)"
expect_not_in "merge commits are left out" "$log" "Merge pull request"
expect_in "header kept" "$log" "All notable changes to Ashdash are documented here."
expect_not_in "no trailing blank line" "$(tail -c 2 CHANGELOG.md | od -An -tx1)" "0a 0a"
expect_eq "no empty-looking sections" "$(grep -c '^### ' CHANGELOG.md)" "7"

echo "== a section of a release with nothing to show"
after_010_lock; c "chore(release): v0.1.0-retry"
expect_prepare_ok "prepare with only skipped commits" 0.1.1 --date 2030-01-02
expect_in "says so" "$(cat CHANGELOG.md)" "_No notable changes._"

echo "== the first release folds the hand-written sections"
new_repo 0.1.0 --lock
git add -A; git commit -q --amend --no-edit
cat >CHANGELOG.md <<'EOF'
# Changelog

Intro of the project.

## [Unreleased]

### Fixed
- a late fix

### Changed
- a late change

## [0.1.0] - unreleased (release candidate)

First public version.

### Added
- the launcher
- the workflows

### Known issues
See the notes.
EOF
git commit -qam "docs: hand-written changelog"
c "feat: something that is not parsed for the first release"
expect_prepare_ok "prepare the first release" 0.1.0 --date 2030-01-02
log="$(cat CHANGELOG.md)"
expect_eq "one section only" "$(grep -c '^## ' CHANGELOG.md)" "1"
expect_in "dated heading" "$log" "## [0.1.0] - 2030-01-02"
expect_not_in "no Unreleased section left" "$log" "[Unreleased]"
expect_not_in "no 'unreleased' draft heading left" "$log" "release candidate)"
expect_in "intro of the draft is kept" "$log" "First public version."
expect_eq "sections: Added, Fixed, Changed, Known issues" "$(grep '^### ' CHANGELOG.md | tr '\n' ',')" "### Added,### Fixed,### Changed,### Known issues,"
expect_in "the late fix moved into Fixed" "$log" "- a late fix"
expect_not_in "history of the first release is not parsed" "$log" "not parsed for the first release"
expect_eq "manifests untouched for the first release" "$(git status --porcelain | sed 's/^...//' | tr '\n' ' ')" "CHANGELOG.md "

echo "== later releases fold hand-written Unreleased entries"
after_010_lock
printf '\n## [Unreleased]\n\n### Fixed\n- a hand-written note\n\n## [0.1.0] - 2029-12-31\n\n### Added\n- old\n' >>CHANGELOG.md
git commit -qam "docs: changelog"
c "fix: generated fix"
expect_prepare_ok "prepare 0.1.1 with an Unreleased section" 0.1.1 --date 2030-01-02
log="$(cat CHANGELOG.md)"
expect_in "generated entry" "$log" "generated fix"
expect_in "hand-written entry" "$log" "- a hand-written note"
expect_eq "old section kept after the new one" "$(grep '^## ' CHANGELOG.md | tr '\n' ',')" "## [0.1.1] - 2030-01-02,## [0.1.0] - 2029-12-31,"

echo "== release candidates"
after_010_lock; c "feat: a"
expect_prepare_ok "prepare 0.2.0-rc.1" 0.2.0-rc.1 --date 2030-01-02
expect_eq "candidate version in the manifest" "$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml)" "0.2.0-rc.1"
if cargo metadata --locked --offline --format-version 1 >/dev/null 2>&1; then pass "cargo accepts a candidate version"; else fail "cargo accepts a candidate version"; fi
expect_in "candidate section" "$(cat CHANGELOG.md)" "## [0.2.0-rc.1] - 2030-01-02"
git add -A; git commit -qm "chore(release): v0.2.0-rc.1"; git tag -a v0.2.0-rc.1 -m rc
c "fix: b"
expect_prepare_ok "prepare the final 0.2.0" 0.2.0 --date 2030-01-09
expect_eq "candidate section replaced by the final one" "$(grep '^## ' CHANGELOG.md | tr '\n' ',')" "## [0.2.0] - 2030-01-09,"
expect_in "final section has the feat of the candidate" "$(cat CHANGELOG.md)" "### Added"
expect_in "and the later fix" "$(cat CHANGELOG.md)" "### Fixed"

echo "== release notes"
after_010_lock; c "fix(core): handle an empty query (#12)"
mkdir -p docs/release-notes
printf '# Install\n\nDownload Ashdash-@VERSION@.dmg from @REPO_URL@/releases.\n' >docs/release-notes/install.md
git add -A; git commit -qm "docs: install block"
expect_prepare_ok "notes without a hand-written introduction" 0.1.1 --date 2030-01-02 --notes-file "$TEST_TMP/notes1.md"
notes="$(cat "$TEST_TMP/notes1.md")"
expect_in "notes: changes" "$notes" "## What's changed"
expect_in "notes: the fix" "$notes" "handle an empty query"
expect_in "notes: compare link" "$notes" "https://example.invalid/me/repo/compare/v0.1.0...v0.1.1"
expect_in "notes: install block rendered" "$notes" "Download Ashdash-0.1.1.dmg from https://example.invalid/me/repo/releases."
expect_not_in "notes: no changelog heading" "$notes" "## [0.1.1]"
git checkout -q .
printf '# Ashdash 0.1.1\n\nHand-written. See [the guide](../USER_GUIDE.md) and [security](../../.github/SECURITY.md).\n' >docs/release-notes/v0.1.1.md
git add -A; git commit -qm "docs: introduction"
expect_prepare_ok "notes with a hand-written introduction" 0.1.1 --date 2030-01-02 --notes-file "$TEST_TMP/notes2.md"
notes="$(cat "$TEST_TMP/notes2.md")"
expect_in "notes: introduction first" "$(head -n 1 "$TEST_TMP/notes2.md")" "# Ashdash 0.1.1"
expect_in "notes: relative link rewritten (docs)" "$notes" "(https://example.invalid/me/repo/blob/v0.1.1/docs/USER_GUIDE.md)"
expect_in "notes: relative link rewritten (root)" "$notes" "(https://example.invalid/me/repo/blob/v0.1.1/.github/SECURITY.md)"
expect_in "notes: still has the changes" "$notes" "handle an empty query"
expect_not_in "notes: no install block next to an introduction" "$notes" "Download Ashdash"

echo "== the first release notes are the introduction"
new_repo 0.1.0 --lock
mkdir -p docs/release-notes
printf '# Ashdash 0.1.0\n\nThe whole story.\n' >docs/release-notes/v0.1.0.md
git add -A; git commit -q --amend --no-edit
expect_prepare_ok "notes of the first release" 0.1.0 --date 2030-01-02 --notes-file "$TEST_TMP/notes3.md"
expect_eq "only the introduction" "$(tr -d '\n' <"$TEST_TMP/notes3.md")" "# Ashdash 0.1.0The whole story."

echo "== determinism"
after_010_lock; c "fix: a"; c "feat: b (#3)"
expect_prepare_ok "first run" 0.2.0 --date 2030-01-02 --notes-file "$TEST_TMP/n1.md"
sum1="$(cat CHANGELOG.md Cargo.lock crates/app/Cargo.toml "$TEST_TMP/n1.md" | shasum | cut -c1-40)"
git checkout -q .
expect_prepare_ok "second run" 0.2.0 --date 2030-01-02 --notes-file "$TEST_TMP/n1.md"
sum2="$(cat CHANGELOG.md Cargo.lock crates/app/Cargo.toml "$TEST_TMP/n1.md" | shasum | cut -c1-40)"
expect_eq "identical output" "$sum2" "$sum1"

echo "== a package that is not part of the workspace keeps its version"
after_010_lock
mkdir -p vendor/other/src
printf '[package]\nname = "other"\nversion = "0.1.0"\nedition = "2024"\n' >vendor/other/Cargo.toml
echo '' >vendor/other/src/lib.rs
sed -i.bak 's|^\[dependencies\]$|[dependencies]\nother = { path = "../../vendor/other" }|' crates/core/Cargo.toml && rm crates/core/Cargo.toml.bak
cargo metadata --offline --format-version 1 >/dev/null 2>&1
git add -A; git commit -qm "build: add a path dependency"
c "fix: a"
expect_prepare_ok "prepare with a third package in the lock file" 0.1.1 --date 2030-01-02
expect_in "the other package is still 0.1.0" "$(grep -A1 'name = "other"' Cargo.lock)" 'version = "0.1.0"'
expect_in "the other dependency line keeps its path form" "$(cat crates/core/Cargo.toml)" 'other = { path = "../../vendor/other" }'

echo "== refusals"
after_010_lock; c "fix: a"
expect_prepare_fails "no version" 64 "missing VERSION"
expect_prepare_fails "bad version" 64 "not a release version" 1.2
expect_prepare_fails "bad date" 64 "date must look like" 0.1.1 --date yesterday
expect_prepare_fails "unknown argument" 64 "unknown argument" 0.1.1 --nope
expect_prepare_fails "two versions" 64 "only one version" 0.1.1 0.1.2
expect_prepare_fails "the tag exists" 65 "already exists" 0.1.0
expect_prepare_fails "lower than the manifests" 65 "lower than" 0.0.9
echo "# change" >>CHANGELOG.md
expect_prepare_fails "dirty tree" 65 "uncommitted changes" 0.1.1
git checkout -q .
git rm -q Cargo.lock; git commit -qm "build: remove the lock file"
expect_prepare_fails "no Cargo.lock" 65 "Cargo.lock not found" 0.1.1
after_010_lock; c "fix: a"
printf '\n## [0.1.1] - 2030-01-01\n\n### Fixed\n- already there\n' >>CHANGELOG.md; git commit -qam "docs: changelog"
expect_prepare_fails "the section already exists" 65 "already has a section" 0.1.1
after_010_lock; c "fix: a"
# a lock file that does not match the manifests must be caught by cargo --locked
sed -i.bak 's/^name = "ashdash-core"/name = "ashdash-coreX"/' Cargo.lock && rm Cargo.lock.bak
git commit -qam "build: break the lock file"
expect_prepare_fails "inconsistent lock file" 65 "Cargo.lock" 0.1.1
GIT_CLIFF=/nonexistent/git-cliff expect_prepare_fails "missing git-cliff" 69 "git-cliff not found" 0.1.1

finish "prepare tests"

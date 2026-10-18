#!/usr/bin/env bash
# Tests of scripts/setup-hooks.sh and the hooks in .githooks/, run for real in throw-away repositories (real `git commit`, real
# `git push` to a local bare remote). cargo is replaced by a recording fake, so nothing is compiled. Needs bash 3.2+, git, awk, rustfmt.
# Secret-looking strings are assembled at run time so this file does not trip the scanner it tests.
set -u
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
TIME_LIMIT=${HOOK_TIME_LIMIT:-3}

# The real toolchain stays reachable after HOME is replaced; git sees no global or system configuration.
export RUSTUP_HOME=${RUSTUP_HOME:-$HOME/.rustup} CARGO_HOME=${CARGO_HOME:-$HOME/.cargo}
TMP=$(mktemp -d "${TMPDIR:-/tmp}/hooks-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/home" "$TMP/bin"
export HOME="$TMP/home" GIT_CONFIG_GLOBAL="$TMP/global.gitconfig" GIT_CONFIG_SYSTEM=/dev/null
printf '[init]\n\tdefaultBranch = main\n[commit]\n\tgpgsign = false\n' >"$GIT_CONFIG_GLOBAL"
GLOBAL_BEFORE=$(cksum <"$GIT_CONFIG_GLOBAL")
export GIT_TERMINAL_PROMPT=0

# A recording fake cargo: logs its arguments and HOME; FAKE_CARGO_FAIL=clippy|test makes that step fail.
cat >"$TMP/bin/cargo" <<'EOF'
#!/usr/bin/env bash
echo "$1 HOME=$HOME" >>"$FAKE_CARGO_LOG"
[ "${FAKE_CARGO_FAIL:-}" = "$1" ] && { echo "fake cargo: $1 failed" >&2; exit 101; }
exit 0
EOF
chmod +x "$TMP/bin/cargo"
export FAKE_CARGO_LOG="$TMP/cargo.log"
PATH="$TMP/bin:$PATH"

NAME=$("$REPO/scripts/format-id.sh" ENV_PREFIX | tr 'A-Z' 'a-z'); NAME=${NAME%_}   # the product name, never written out in this file

# new_repo <name>: a clone-like sandbox with the hooks installed through setup-hooks.sh; sets S.
new_repo() {
  S="$TMP/$1"
  mkdir -p "$S/scripts" "$S/crates/core/src" "$S/crates/core/tests" "$S/crates/app" "$S/docs"
  git init -q "$S"
  cp -R "$REPO/.githooks" "$S/.githooks"
  cp "$REPO/scripts/commit-lint.sh" "$REPO/scripts/format-id.sh" "$REPO/scripts/setup-hooks.sh" "$S/scripts/"
  cp "$REPO/crates/core/src/format_ids.rs" "$S/crates/core/src/"
  cp "$REPO/crates/core/tests/names.rs" "$S/crates/core/tests/"
  cp "$REPO/rustfmt.toml" "$S/"
  printf '[package]\nname = "t"\nedition = "2024"\n' >"$S/crates/app/Cargo.toml"
  printf 'pub fn f() -> u32 {\n    1\n}\n' >"$S/crates/app/lib.rs"
  git -C "$S" config user.email 'tester@users.noreply.github.com'
  git -C "$S" config user.name 'Tester'
  git -C "$S" add -A
  git -C "$S" commit -q --no-verify -m 'Initial commit before the convention'
  "$S/scripts/setup-hooks.sh" >/dev/null
}
g() { git -C "$S" "$@"; }
commits() { g rev-list --count HEAD; }
# try <message> [git commit options]: commits what is staged; sets OUT and RC.
try() { local m=$1; shift; OUT=$(g commit -q -m "$m" "$@" 2>&1); RC=$?; }
stage() { # stage <path> <content>
  mkdir -p "$S/$(dirname "$1")"; printf '%s\n' "$2" >"$S/$1"; g add -- "$1"
}
timed() { # timed <command...>: runs it, sets SECS (wall clock, 3 decimals)
  local TIMEFORMAT=%3R
  SECS=$( { time "$@" >/dev/null 2>&1; } 2>&1 )
}
refused() { # refused <commit count before> <expected text> <description>: the last try was refused and created no commit
  local before=$1 text=$2 desc=$3
  expect_rc 1 "$desc" "$RC" "$OUT"
  expect_contains "$desc" "$OUT" "$text"
  [ "$(commits)" = "$before" ] && pass || fail "$desc (a commit was created)"
}

# ============================================================ setup-hooks.sh
new_repo setup
g config --local --unset core.hooksPath
out=$("$S/scripts/setup-hooks.sh" --status 2>&1); expect_contains "status before installing" "$out" "not installed"
out=$("$S/scripts/setup-hooks.sh" 2>&1); rc=$?; expect_rc 0 "install" "$rc" "$out"
[ "$(g config --local core.hooksPath)" = ".githooks" ] && pass || fail "install sets core.hooksPath to .githooks"
out=$("$S/scripts/setup-hooks.sh" 2>&1); rc=$?; expect_rc 0 "install twice" "$rc" "$out"; expect_contains "install is idempotent" "$out" "already installed"
[ "$(g config --local --get-all core.hooksPath | wc -l | tr -d ' ')" = 1 ] && pass || fail "install twice leaves one value"
out=$("$S/scripts/setup-hooks.sh" --status 2>&1); expect_contains "status after installing" "$out" "installed (core.hooksPath"
[ "$(cksum <"$GIT_CONFIG_GLOBAL")" = "$GLOBAL_BEFORE" ] && pass || fail "setup-hooks must not touch the global git configuration"
out=$("$S/scripts/setup-hooks.sh" --uninstall 2>&1); rc=$?; expect_rc 0 "uninstall" "$rc" "$out"
[ -z "$(g config --local --get core.hooksPath || true)" ] && pass || fail "uninstall removes core.hooksPath"
out=$("$S/scripts/setup-hooks.sh" --uninstall 2>&1); rc=$?; expect_rc 0 "uninstall twice" "$rc" "$out"
g config --local core.hooksPath custom-hooks
out=$("$S/scripts/setup-hooks.sh" 2>&1); rc=$?; expect_rc 1 "install refuses to replace another hooksPath" "$rc" "$out"
out=$("$S/scripts/setup-hooks.sh" --uninstall 2>&1); expect_contains "uninstall leaves another hooksPath alone" "$out" "left alone"
[ "$(g config --local core.hooksPath)" = "custom-hooks" ] && pass || fail "another hooksPath is kept"
out=$("$S/scripts/setup-hooks.sh" --force 2>&1); rc=$?; expect_rc 0 "install --force" "$rc" "$out"
[ "$(g config --local core.hooksPath)" = ".githooks" ] && pass || fail "--force replaces it"
out=$("$S/scripts/setup-hooks.sh" --bogus 2>&1); rc=$?; expect_rc 2 "unknown option" "$rc" "$out"
out=$(cd "$TMP" && "$S/scripts/setup-hooks.sh" 2>&1); rc=$?; expect_rc 0 "works from another directory" "$rc" "$out"
[ "$(cksum <"$GIT_CONFIG_GLOBAL")" = "$GLOBAL_BEFORE" ] && pass || fail "the global configuration is still untouched"

# ============================================================ commit-msg
new_repo msg
stage notes.md 'one'; n=$(commits)
try 'Add some notes'; refused "$n" "Conventional" "a bad message is rejected"
expect_contains "the rejection shows an example" "$OUT" "Valid examples"
expect_contains "the rejection says how to retry" "$OUT" "git commit -e -F"
try 'feat(docs): add some notes'; expect_rc 0 "a good message is accepted" "$RC" "$OUT"
[ "$(commits)" = $((n + 1)) ] && pass || fail "a good message creates the commit"
stage notes.md 'two'; n=$(commits)
try "docs: compare with $NAME"; refused "$n" "must not appear" "the product name in a message is rejected"
stage notes.md 'three'; n=$(commits)
try 'Add notes' --no-verify; expect_rc 0 "--no-verify skips the hooks" "$RC" "$OUT"

# ============================================================ pre-commit: the normal path
new_repo clean
printf 'pub fn f() -> u32 {\n    2\n}\n' >"$S/crates/app/lib.rs"; g add crates/app/lib.rs
stage docs/note.md 'a line'
n=$(commits)
timed git -C "$S" commit -q -m 'fix(app): return two'
[ "$(commits)" = $((n + 1)) ] && pass || fail "a clean change commits"
echo "timing: clean commit (rustfmt on 1 file + all checks) took ${SECS}s"
awk -v s="$SECS" -v l="$TIME_LIMIT" 'BEGIN { exit !(s < l) }' && pass || fail "a clean small commit takes under ${TIME_LIMIT}s (took ${SECS}s)"
printf 'pub fn f() -> u32 {\n    3\n}\n' >"$S/crates/app/lib.rs"; g add crates/app/lib.rs
OUT=$(cd "$S" && .githooks/pre-commit 2>&1); RC=$?
expect_rc 0 "pre-commit run directly" "$RC" "$OUT"
for what in rustfmt "file sizes" "whitespace" "secret scan" "names guard" "all checks passed"; do expect_contains "pre-commit prints what it checks: $what" "$OUT" "$what"; done
g reset -q --hard
OUT=$(g commit -q --allow-empty -m 'chore: empty again' 2>&1); expect_rc 0 "an empty commit is fine" "$?" "$OUT"

# ============================================================ pre-commit: rustfmt
new_repo fmt
stage crates/app/lib.rs 'pub fn f()->u32{1}'; n=$(commits)
try 'style: break the format'; refused "$n" "not formatted" "unformatted Rust is rejected"
expect_contains "rustfmt rejection names the fix" "$OUT" "cargo fmt --all"
printf 'pub fn f() -> u32 {\n    7\n}\n' >"$S/crates/app/lib.rs"; g add crates/app/lib.rs
printf 'pub fn f()->u32{9}\n' >"$S/crates/app/lib.rs"   # unstaged, unformatted: the index is what counts
try 'style: keep the index formatted'; expect_rc 0 "rustfmt checks the staged content, not the working tree" "$RC" "$OUT"

# ============================================================ pre-commit: whitespace and conflict markers
new_repo ws
stage docs/a.md 'text with trailing space  '; n=$(commits)
try 'docs: add a'; refused "$n" "trailing whitespace" "trailing whitespace on an added line is rejected"
g reset -q
printf 'old line  \nsecond\n' >"$S/docs/b.md"; g add docs/b.md; g commit -q --no-verify -m 'docs: b with old whitespace'
printf 'old line  \nsecond changed\n' >"$S/docs/b.md"; g add docs/b.md
try 'docs: change b'; expect_rc 0 "whitespace on lines you did not change is not your problem" "$RC" "$OUT"
stage docs/c.md "$(printf 'one\n<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> branch')"; n=$(commits)
try 'docs: add c'; refused "$n" "conflict marker" "conflict markers are rejected"

# ============================================================ pre-commit: file size
new_repo size
big() { mkdir -p "$S/$(dirname "$1")"; dd if=/dev/zero of="$S/$1" bs=1024 count="${2:-1200}" 2>/dev/null; g add -- "$1"; }
mkdir -p "$S/docs/assets" "$S/crates/app/resources/fonts"
big data/dump.bin; n=$(commits)
try 'chore: add a dump'; refused "$n" "too large" "a file over 1 MiB is rejected"
g reset -q; rm -rf "$S/data"
big docs/assets/screenshot.png;                        try 'docs: add a screenshot'; expect_rc 0 "a large image under docs/assets is allowed" "$RC" "$OUT"
big crates/app/resources/fonts/Face-Regular.ttf;       try 'build: add a font'; expect_rc 0 "a font is allowed" "$RC" "$OUT"
big docs/other/shot.png;                               n=$(commits); try 'docs: add elsewhere'; refused "$n" "too large" "an image outside docs/assets is rejected"
OUT=$(ALLOW_LARGE_FILES=1 g commit -q -m 'docs: add elsewhere' 2>&1); expect_rc 0 "ALLOW_LARGE_FILES=1 lets a large file through" "$?" "$OUT"
printf 'x\n' >"$S/small.bin"; dd if=/dev/zero bs=1024 count=1024 2>/dev/null >>"$S/small.bin"; g add small.bin
try 'chore: just over'; [ "$RC" = 1 ] && pass || fail "1 MiB plus one byte is over the limit"
g reset -q
dd if=/dev/zero of="$S/exact.bin" bs=1024 count=1024 2>/dev/null; g add exact.bin
try 'chore: exactly 1 MiB'; expect_rc 0 "exactly 1 MiB is fine" "$RC" "$OUT"

# ============================================================ pre-commit: secrets
new_repo secrets
fill() { printf '%*s' "$2" '' | tr ' ' "$1"; }
secret_case() { # secret_case <description> <content>
  g reset -q; stage docs/leak.md "$2"; local n; n=$(commits)
  try 'docs: add leak'; refused "$n" "looks like" "secret scan rejects $1"
  expect_lacks "the secret value is not echoed ($1)" "$OUT" "$(printf '%s' "$2" | cut -c1-24)"
}
secret_case "a private key"    "$(printf '%s%s\n%s' '-----BEGIN ' 'RSA PRIVATE KEY-----' 'MIIEowIBAAKCAQEA')"
secret_case "an OpenSSH key"   "$(printf '%s%s' '-----BEGIN ' 'OPENSSH PRIVATE KEY-----')"
secret_case "a GitHub token"   "token = gh""p_$(fill a 36)"
secret_case "a fine-grained GitHub token" "t=github""_pat_$(fill b 30)"
secret_case "an AWS key id"    "id: AK""IA$(fill A 16)"
secret_case "a Slack token"    "slack: xo""xb-1234567890-$(fill c 12)"
secret_case "a Google API key" "key=AI""za$(fill d 35)"
g reset -q; stage docs/ok.md 'ghp_ is the prefix of a GitHub token; a short ghp_abc is not one'; try 'docs: mention the prefix'; expect_rc 0 "talking about token prefixes is fine" "$RC" "$OUT"
stage docs/mark.md "key=AI""za$(fill e 35) # secret-scan:allow"; try 'docs: allowed example'; expect_rc 0 "the allow marker silences one line" "$RC" "$OUT"
for fname in .env prod/.env.local keys/server.pem id_rsa deploy/id_ed25519 cert.p12 credentials.json; do
  g reset -q; stage "$fname" 'x'; n=$(commits)
  try 'chore: add a file'; refused "$n" "secret-looking file name" "secret scan rejects the file name $fname"
done
g reset -q; stage .env.example 'KEY='; try 'docs: add the example env'; expect_rc 0 ".env.example is fine" "$RC" "$OUT"
stage id_rsa.pub 'public'; try 'docs: add a public key'; expect_rc 0 "a public key file is fine" "$RC" "$OUT"
# gitleaks, when it is installed, replaces the built-in patterns; a fake one proves the hook calls it
mkdir -p "$TMP/gl"; printf '#!/usr/bin/env bash\necho "gitleaks $*" >>"$FAKE_CARGO_LOG"\n[ "${FAKE_GITLEAKS_FAIL:-}" = 1 ] && { echo "leaks found" >&2; exit 1; }\nexit 0\n' >"$TMP/gl/gitleaks"; chmod +x "$TMP/gl/gitleaks"
g reset -q; : >"$FAKE_CARGO_LOG"; stage docs/gl.md 'text'
OUT=$(PATH="$TMP/gl:$PATH" g commit -q -m 'docs: with gitleaks' 2>&1); RC=$?
expect_rc 0 "gitleaks present and happy" "$RC" "$OUT"; expect_contains "the hook runs gitleaks on the staged changes" "$(cat "$FAKE_CARGO_LOG")" "--staged"
stage docs/gl2.md 'text'; n=$(commits)
OUT=$(FAKE_GITLEAKS_FAIL=1 PATH="$TMP/gl:$PATH" g commit -q -m 'docs: with gitleaks again' 2>&1); RC=$?; refused "$n" "leaks found" "gitleaks findings block the commit"

# ============================================================ pre-commit: names, e-mail
new_repo names
stage notes.txt "compare with $NAME"; n=$(commits)
try 'docs: add notes'; refused "$n" "must not appear" "the product name in a normal file is rejected"
expect_lacks "the rejection does not print the name" "$OUT" "$NAME"
g reset -q; stage notes.txt "compare with ${NAME}o"; try 'docs: add notes'; refused "$n" "must not appear" "the retired working name is rejected"
g reset -q; stage "docs/${NAME}-notes.md" 'x'; try 'docs: add notes'; refused "$n" "path contains" "a path with the name is rejected"
g reset -q; stage docs/WORKFLOWS.md "the ${NAME}workflow extension"; try 'docs: list the format'; expect_rc 0 "the allowed files may mention the format identifiers" "$RC" "$OUT"
stage docs/WORKFLOWS.md "the ${NAME}owf extension"; n=$(commits); try 'docs: list the format'; refused "$n" "must not appear" "the retired name is rejected even in an allowed file"
g reset -q
OUT=$(g commit -q --allow-empty -m 'chore: noreply' 2>&1); expect_lacks "no e-mail warning for a noreply address" "$OUT" "will be public"
g config user.email 'someone@example.com'; stage notes2.txt 'fine'
try 'docs: personal address'; expect_rc 0 "a personal e-mail only warns" "$RC" "$OUT"; expect_contains "the e-mail warning" "$OUT" "will be public"

# ============================================================ pre-commit: shellcheck and actionlint (stubs; the real ones run in CI)
new_repo lint
mkdir -p "$TMP/stub-fail" "$TMP/stub-ok"
for t in shellcheck actionlint; do
  printf '#!/bin/sh\necho "%s found a problem" >&2\nexit 1\n' "$t" >"$TMP/stub-fail/$t"
  printf '#!/bin/sh\nexit 0\n' >"$TMP/stub-ok/$t"
done
chmod +x "$TMP"/stub-*/*
stage tools/x.sh $'#!/usr/bin/env bash\necho hi'; n=$(commits)
OUT=$(PATH="$TMP/stub-fail:$PATH" g commit -q -m 'build: add x' 2>&1); RC=$?; refused "$n" "shellcheck found a problem" "a failing shellcheck blocks the commit"
OUT=$(PATH="$TMP/stub-ok:$PATH" g commit -q -m 'build: add x' 2>&1); expect_rc 0 "a passing shellcheck lets it through" "$?" "$OUT"
stage tools/y.sh $'#!/usr/bin/env bash\necho hi'; stage docs/script.sh $'#!/bin/sh\necho hi'
OUT=$(PATH="$TMP/stub-fail:$PATH" g commit -q -m 'build: add y' 2>&1); RC=$?; expect_rc 1 "a second script is checked too" "$RC" "$OUT"
g reset -q; rm -f "$S/tools/y.sh"
stage docs/only.sh $'#!/bin/sh\necho hi'
OUT=$(PATH="$TMP/stub-fail:$PATH" g commit -q -m 'docs: add a doc script' 2>&1); expect_rc 0 "scripts under docs/ are out of scope (as in CI)" "$?" "$OUT"
stage .github/workflows/w.yml $'name: w\non: push\njobs: {}'; n=$(commits)
OUT=$(PATH="$TMP/stub-fail:$PATH" g commit -q -m 'ci: add w' 2>&1); RC=$?; refused "$n" "actionlint found a problem" "a failing actionlint blocks the commit"
OUT=$(PATH="$TMP/stub-ok:$PATH" g commit -q -m 'ci: add w' 2>&1); expect_rc 0 "a passing actionlint lets it through" "$?" "$OUT"
# not installed: the check is skipped and says so (only meaningful when the tool is really absent from a minimal PATH)
MIN_PATH="/usr/bin:/bin:/usr/sbin:/sbin:$(dirname "$(command -v rustfmt)"):$TMP/bin"
if ! PATH="$MIN_PATH" command -v shellcheck >/dev/null 2>&1 && ! PATH="$MIN_PATH" command -v actionlint >/dev/null 2>&1; then
  stage tools/z.sh $'#!/usr/bin/env bash\necho hi'; stage .github/workflows/z.yml $'name: z\non: push\njobs: {}'
  OUT=$(PATH="$MIN_PATH" g commit -q -m 'build: add z' 2>&1); RC=$?
  expect_rc 0 "missing shellcheck and actionlint do not block" "$RC" "$OUT"
  expect_contains "the skipped shellcheck is named" "$OUT" "shellcheck ... skipped"
  expect_contains "the skipped actionlint is named" "$OUT" "actionlint ... skipped"
else
  echo "note: shellcheck or actionlint is on the minimal PATH here; the 'not installed' case was not exercised"
fi

# ============================================================ pre-push
new_repo push
git init -q --bare "$TMP/origin.git"
g remote add origin "$TMP/origin.git"
g push -q --no-verify origin main 2>&1 | tail -n 2
g checkout -q -b topic
stage notes.md 'a'; try 'feat: add a'
: >"$FAKE_CARGO_LOG"
OUT=$(g push -u origin topic 2>&1); RC=$?
expect_rc 0 "a push of good commits goes through" "$RC" "$OUT"
expect_contains "pre-push runs clippy" "$(cat "$FAKE_CARGO_LOG")" "clippy"
expect_contains "pre-push runs the tests" "$(cat "$FAKE_CARGO_LOG")" "test"
expect_lacks "cargo does not see the real HOME" "$(cat "$FAKE_CARGO_LOG")" "HOME=$HOME "
expect_contains "cargo runs with a throw-away HOME" "$(cat "$FAKE_CARGO_LOG")" "pre-push-home"
expect_contains "pre-push prints what it checks" "$OUT" "Conventional Commits check of the pushed commits"
[ "$(git -C "$TMP/origin.git" rev-parse topic)" = "$(g rev-parse topic)" ] && pass || fail "the branch arrived"
stage notes.md 'b'; try 'feat: add b'
: >"$FAKE_CARGO_LOG"; OUT=$(SKIP_PREPUSH=1 g push origin topic 2>&1); RC=$?
expect_rc 0 "SKIP_PREPUSH=1 pushes" "$RC" "$OUT"
[ ! -s "$FAKE_CARGO_LOG" ] && pass || fail "SKIP_PREPUSH=1 skips cargo" "$(cat "$FAKE_CARGO_LOG")"
stage notes.md 'c'; try 'feat: add c'
: >"$FAKE_CARGO_LOG"; OUT=$(FAKE_CARGO_FAIL='clippy' g push origin topic 2>&1); RC=$?
expect_rc 1 "a clippy failure blocks the push" "$RC" "$OUT"; expect_contains "the clippy failure is explained" "$OUT" "clippy found problems"
[ "$(git -C "$TMP/origin.git" rev-parse topic)" != "$(g rev-parse topic)" ] && pass || fail "nothing arrived after the clippy failure"
OUT=$(FAKE_CARGO_FAIL='test' g push origin topic 2>&1); RC=$?
expect_rc 1 "a test failure blocks the push" "$RC" "$OUT"; expect_contains "the test failure is explained" "$OUT" "tests failed"
OUT=$(g push origin topic 2>&1); expect_rc 0 "and the push works once cargo is happy" "$?" "$OUT"
stage notes.md 'd'; try 'wip stuff' --no-verify
OUT=$(g push origin topic 2>&1); RC=$?
expect_rc 1 "a push with a bad commit message is refused" "$RC" "$OUT"; expect_contains "the bad commit is named" "$OUT" "wip stuff"
OUT=$(SKIP_PREPUSH=1 g push origin topic 2>&1); RC=$?; expect_rc 1 "SKIP_PREPUSH=1 does not skip the message check" "$RC" "$OUT"
OUT=$(g push --no-verify origin topic 2>&1); expect_rc 0 "--no-verify skips everything" "$?" "$OUT"
# a new branch is judged only by its own commits, not by the old history under it
g checkout -q -b second topic~0
g reset -q --hard HEAD~1
stage other.md 'x'; try 'fix: other'
OUT=$(g push -u origin second 2>&1); expect_rc 0 "a new branch: only commits not on the remote are checked" "$?" "$OUT"
g checkout -q -b third main
stage third.md 'x'; try 'Third thing' --no-verify
OUT=$(g push -u origin third 2>&1); RC=$?; expect_rc 1 "a new branch with a bad commit is refused" "$RC" "$OUT"
# a brand-new remote (a fork you just added) has no remote-tracking branches: the old history must not be judged
git init -q --bare "$TMP/fork.git"; g remote add fork "$TMP/fork.git"
g checkout -q -b fourth main
stage fourth.md 'x'; try 'fix: fourth'
OUT=$(g push -u fork fourth 2>&1); expect_rc 0 "a first push to a new remote judges only commits no remote has" "$?" "$OUT"
g tag v9.9.9 main
OUT=$(g push origin v9.9.9 2>&1); expect_rc 0 "pushing a tag is not linted" "$?" "$OUT"
OUT=$(g push origin :second 2>&1); expect_rc 0 "deleting a branch is not linted" "$?" "$OUT"

# ============================================================ the pre-commit framework config
if command -v pre-commit >/dev/null 2>&1; then
  OUT=$(cd "$REPO" && pre-commit validate-config .pre-commit-config.yaml 2>&1); expect_rc 0 ".pre-commit-config.yaml is valid" "$?" "$OUT"
else
  echo "note: the pre-commit framework is not installed; .pre-commit-config.yaml was not validated"
fi

summary "hooks"

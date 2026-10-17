#!/bin/bash
# Runs on the GUEST (piped through ssh by golden-setup.sh): idempotent provisioning of the golden image.
# usage: bash -s -- <rust-toolchain> <admin-password>
set -euo pipefail
TOOLCHAIN=${1:-stable}
PW=${2:-admin}
say() { printf '[provision] %s\n' "$*"; }
if sudo -n true 2>/dev/null; then SUDO=(sudo -n); else SUDO=(sudo -S -p ''); fi
as_root() { if [ "${SUDO[1]}" = "-n" ]; then "${SUDO[@]}" "$@"; else printf '%s\n' "$PW" | "${SUDO[@]}" "$@"; fi; }

say "macOS $(sw_vers -productVersion) ($(uname -m))"

# 1. Xcode Command Line Tools (compiler, git, codesign helpers). Needed by cargo for linking.
if xcode-select -p >/dev/null 2>&1 && [ -x "$(xcode-select -p)/usr/bin/clang" ]; then
  say "command line tools present: $(xcode-select -p)"
else
  say "installing command line tools via softwareupdate (several minutes)"
  touch /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress-file
  label=$(softwareupdate -l 2>/dev/null | sed -n 's/^\* Label: \(Command Line Tools.*\)$/\1/p' | sort -V | tail -1)
  [ -n "$label" ] || label=$(softwareupdate -l 2>/dev/null | grep -E '^\s*\* .*Command Line Tools' | sed 's/^[ *]*//' | sort -V | tail -1)
  [ -n "$label" ] || { say "no CLT label found; install manually with 'xcode-select --install' in the VM window"; exit 1; }
  as_root softwareupdate -i "$label" --verbose
  rm -f /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress-file
  as_root xcode-select --switch /Library/Developer/CommandLineTools
fi

# 2. Rust (rustup, minimal profile) matching the host toolchain, plus the universal-build target.
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v rustup >/dev/null 2>&1; then
  say "installing rustup"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
fi
say "installing toolchain $TOOLCHAIN"
rustup toolchain install "$TOOLCHAIN" --profile minimal -c clippy -c rustfmt -t x86_64-apple-darwin -t aarch64-apple-darwin
rustup default "$TOOLCHAIN"
rustc --version
cargo --version

# 3. No sleep, no screen saver, no lock screen, no password after sleep.
say "disabling sleep / screen lock"
as_root pmset -a sleep 0 displaysleep 0 disksleep 0 powernap 0 womp 0 || true
defaults write com.apple.screensaver idleTime -int 0
defaults -currentHost write com.apple.screensaver idleTime -int 0
defaults write com.apple.screensaver askForPassword -int 0
defaults write com.apple.screensaver askForPasswordDelay -int 0
sysadminctl -screenLock off -password "$PW" 2>&1 | sed 's/^/[sysadminctl] /' || true
as_root defaults write /Library/Preferences/com.apple.loginwindow DisableScreenLockImmediate -bool true || true
# Crash reporter / first-run noise must not cover the screen during UI tests.
defaults write com.apple.CrashReporter DialogType none
defaults write com.apple.Terminal SecureKeyboardEntry -bool false
as_root softwareupdate --schedule off >/dev/null 2>&1 || true

# 4. Auto-login (the cirruslabs images ship with it; verify, never assume).
auto=$(defaults read /Library/Preferences/com.apple.loginwindow autoLoginUser 2>/dev/null || true)
if [ "$auto" = "$(id -un)" ] && [ -f /etc/kcpassword ]; then
  say "auto-login already set for $auto"
else
  say "enabling auto-login for $(id -un)"
  as_root sysadminctl -autologin set -userName "$(id -un)" -password "$PW" 2>&1 | sed 's/^/[sysadminctl] /' || true
  auto=$(defaults read /Library/Preferences/com.apple.loginwindow autoLoginUser 2>/dev/null || true)
  [ "$auto" = "$(id -un)" ] || say "WARNING: auto-login not confirmed (autoLoginUser='$auto'); log in once in the VM window"
fi

# 5. Login keychain unlocked and never auto-locking (codesign / security CLI in tests).
kc="$HOME/Library/Keychains/login.keychain-db"
if [ -f "$kc" ]; then
  security unlock-keychain -p "$PW" "$kc" 2>/dev/null || say "note: could not unlock login keychain"
  security set-keychain-settings "$kc" 2>/dev/null || true
fi

# 6. Remote login (sshd) must stay on; make a quiet shell environment for non-interactive ssh.
touch "$HOME/.hushlogin"
say "done"

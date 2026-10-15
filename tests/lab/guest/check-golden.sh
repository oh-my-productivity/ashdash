#!/bin/bash
# Runs on the GUEST (via `golden-setup.sh verify`): report what the golden image can and cannot do.
export PATH="$HOME/.cargo/bin:$PATH"
row() { printf '%-26s %s\n' "$1" "$2"; }
row macOS "$(sw_vers -productVersion) $(sw_vers -buildVersion) $(uname -m)"
row sip "$(csrutil status 2>&1 | head -1)"
row clt "$(xcode-select -p 2>&1)"
row clang "$(clang --version 2>&1 | head -1)"
row rustc "$(rustc --version 2>&1)"
row cargo "$(cargo --version 2>&1)"
row targets "$(rustup target list --installed 2>&1 | tr '\n' ' ')"
row clippy/rustfmt "$(cargo clippy --version 2>&1 | head -1) / $(rustfmt --version 2>&1 | head -1)"
row sleep "$(pmset -g | grep -E '^ *(sleep|displaysleep)' | tr -s ' ' | tr '\n' ';')"
row autologin "$(defaults read /Library/Preferences/com.apple.loginwindow autoLoginUser 2>&1)"
row console-user "$(stat -f %Su /dev/console)"
row screenlock "$(sysadminctl -screenLock status 2>&1 | head -1)"
row "login item/ssh user" "$(id -un)"
gui=$("$(dirname "$0")/gui-exec" --timeout 30 -- /usr/bin/id -un 2>&1)
row "gui-exec (Terminal)" "${gui:-<no output>}"
se=$("$(dirname "$0")/gui-exec" --timeout 30 -- osascript -e 'tell application "System Events" to get name of first process whose frontmost is true' 2>&1)
row "System Events" "$se"
case "$se" in *1002*|*"not allowed"*|*-1743*|*"not authorized"*) row "=> TCC" "NOT GRANTED: do the manual steps (docs/LAB.md)";; *) row "=> TCC" "Accessibility+Automation look granted";; esac
shot=$(mktemp "${TMPDIR:-/tmp}/shot.XXXXXX").png
"$(dirname "$0")/gui-exec" --timeout 30 -- screencapture -x "$shot" >/dev/null 2>&1
row screencapture "$( [ -s "$shot" ] && echo "ok ($(stat -f %z "$shot") bytes)" || echo FAILED )"
rm -f "$shot"

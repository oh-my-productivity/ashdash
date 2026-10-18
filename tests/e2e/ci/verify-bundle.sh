#!/usr/bin/env bash
# Verify a universal Ashdash.app and its DMG as produced by packaging/build-app.sh and make-dmg.sh.
#   tests/e2e/ci/verify-bundle.sh [dist-dir]       (default: dist)
# Checks: both architectures, signature (ad-hoc is fine), Info.plist syntax, the DMG checksum and image integrity, the app inside the
# mounted image, and that the binary of the native architecture starts (`ashdash paths` with a throw-away HOME).
set -uo pipefail
cd "$(dirname "$0")/../../.." || exit 1
dist=${1:-dist}
fails=0
ok() { printf '  ok   %s\n' "$1"; }
bad() { printf '  FAIL %s\n' "$1"; fails=$((fails + 1)); }
check() { local d=$1; shift; if "$@" >/dev/null 2>&1; then ok "$d"; else bad "$d"; fi; }

app=$dist/Ashdash.app
dmg=$(ls "$dist"/Ashdash-*.dmg 2>/dev/null | head -1)
[ -d "$app" ] || { echo "verify-bundle: $app not found" >&2; exit 1; }
[ -n "$dmg" ] || { echo "verify-bundle: no DMG in $dist" >&2; exit 1; }
bin=$app/Contents/MacOS/ashdash

archs=$(lipo -archs "$bin" 2>&1)
case " $archs " in *" arm64 "*) case " $archs " in *" x86_64 "*) ok "universal binary ($archs)" ;; *) bad "no x86_64 slice ($archs)" ;; esac ;; *) bad "no arm64 slice ($archs)" ;; esac
check "Info.plist is valid" plutil -lint "$app/Contents/Info.plist"
check "codesign --verify --deep --strict" codesign --verify --deep --strict "$app"
check "version in Info.plist matches the DMG name" bash -c "[ \"$dist/Ashdash-\$(plutil -extract CFBundleShortVersionString raw -o - '$app/Contents/Info.plist').dmg\" = '$dmg' ]"
check "DMG checksum file matches" bash -c "cd '$dist' && shasum -a 256 -c '$(basename "$dmg").sha256'"
check "hdiutil verify" hdiutil verify "$dmg"
mnt=$(mktemp -d "${TMPDIR:-/tmp}/verify-dmg.XXXXXX")
if hdiutil attach -nobrowse -readonly -noverify -mountpoint "$mnt" "$dmg" >/dev/null 2>&1; then
  [ -d "$mnt/Ashdash.app" ] && ok "the DMG contains Ashdash.app" || bad "no Ashdash.app in the DMG"
  [ -L "$mnt/Applications" ] && ok "the DMG has the Applications link" || bad "no Applications link in the DMG"
  check "app inside the DMG passes codesign --verify" codesign --verify --deep --strict "$mnt/Ashdash.app"
  hdiutil detach "$mnt" >/dev/null 2>&1 || hdiutil detach -force "$mnt" >/dev/null 2>&1
else
  bad "could not mount the DMG"
fi
rmdir "$mnt" 2>/dev/null
home=$(mktemp -d "${TMPDIR:-/tmp}/verify-home.XXXXXX")
out=$(HOME=$home "$bin" paths 2>&1); rc=$?
rm -rf "$home"
[ "$rc" = 0 ] && ok "the bundled binary starts (ashdash paths)" || bad "ashdash paths failed: $(echo "$out" | head -3)"
ls -l "$dist"
[ "$fails" = 0 ] && echo "verify-bundle: OK" || echo "verify-bundle: $fails FAILED"
exit "$fails"

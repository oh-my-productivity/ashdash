#!/usr/bin/env bash
# Fast, offline checks of the packaging inputs (no cargo build, no signing, no network).
# Run by CI before a release and by hand after touching anything under packaging/.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

fail=0
check() { # check DESCRIPTION COMMAND...
  local what="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    printf 'ok   %s\n' "$what"
  else
    printf 'FAIL %s\n' "$what"
    fail=1
  fi
}
equals() { [ "$1" = "$2" ]; }

work="$(mktemp -d "${TMPDIR:-/tmp}/ashdash-selftest.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# --- scripts parse (and are shellcheck-clean when shellcheck exists)
for script in "$PACKAGING_DIR"/*.sh "$PACKAGING_DIR"/icon/*.sh "$PACKAGING_DIR"/homebrew/*.sh; do
  check "bash -n $(basename "$script")" bash -n "$script"
done
if command -v shellcheck >/dev/null 2>&1; then
  check "shellcheck" shellcheck -x -P SCRIPTDIR "$PACKAGING_DIR"/*.sh "$PACKAGING_DIR"/icon/*.sh "$PACKAGING_DIR"/homebrew/*.sh
else
  printf 'skip shellcheck (not installed)\n'
fi

# --- brand / version helpers
check "brand NAME" equals "$(brand_const NAME)" "Ashdash"
check "brand BUNDLE_ID" equals "$(brand_const BUNDLE_ID)" "io.github.oh-my-productivity.ashdash"
check "brand URL_SCHEME is not reference" equals "$(brand_const URL_SCHEME)" "ashdash"
check "version looks like semver" bash -c "printf %s '$(app_version)' | grep -Eq '^[0-9]+\\.[0-9]+\\.[0-9]+'"
check "BUILD_NUMBER override" equals "$(BUILD_NUMBER=77 build_number)" "77"
check "adhoc identity default" equals "$(unset CODESIGN_IDENTITY; signing_identity)" "-"
check "real identity passes through" equals "$(CODESIGN_IDENTITY='Developer ID Application: X' signing_identity)" "Developer ID Application: X"
printf 'a @B@ @C@\n' >"$work/t.txt"
check "render_template replaces" equals "$(render_template "$work/t.txt" B=1 C=2)" "a 1 2"
check "render_template rejects leftovers" bash -c ". '$PACKAGING_DIR/lib.sh'; ! (render_template '$work/t.txt' B=1) 2>/dev/null"

# --- Info.plist and entitlements
plist="$work/Info.plist"
render_template "$REPO_ROOT/crates/app/resources/Info.plist.template" \
  "EXECUTABLE=$(brand_const SLUG)" "BUNDLE_ID=$(brand_const BUNDLE_ID)" "NAME=$(brand_const NAME)" \
  "VERSION=$(app_version)" "BUILD=1" "MIN_OS=12.0" "URL_SCHEME=$(brand_const URL_SCHEME)" "YEAR=2026" >"$plist"
check "Info.plist is valid" plutil -lint "$plist"
check "bundle id" equals "$(plutil -extract CFBundleIdentifier raw -o - "$plist")" "io.github.oh-my-productivity.ashdash"
check "LSUIElement (no Dock icon)" equals "$(plutil -extract LSUIElement raw -o - "$plist")" "true"
check "URL scheme ashdash" equals "$(plutil -extract CFBundleURLTypes.0.CFBundleURLSchemes.0 raw -o - "$plist")" "ashdash"
check "only one URL scheme" equals "$(plutil -extract CFBundleURLTypes.0.CFBundleURLSchemes raw -o - "$plist")" "1"
check "document types: 3" equals "$(plutil -extract CFBundleDocumentTypes raw -o - "$plist")" "3"
check "documents never outrank the reference launcher" bash -c "[ \"\$(plutil -extract CFBundleDocumentTypes.0.LSHandlerRank raw -o - '$plist')\" = Alternate ]"
check "AppleEvents usage string" plutil -extract NSAppleEventsUsageDescription raw -o - "$plist"
check "AppleScript enabled (docs/AUTOMATION.md needs it)" equals "$(plutil -extract NSAppleScriptEnabled raw -o - "$plist")" "true"
check "scripting definition named in Info.plist" equals "$(plutil -extract OSAScriptingDefinition raw -o - "$plist")" "Ashdash.sdef"
check "the sdef that build-app.sh installs exists and is well-formed" bash -c "[ -s '$REPO_ROOT/crates/app/resources/Ashdash.sdef' ] && xmllint --noout --nonet '$REPO_ROOT/crates/app/resources/Ashdash.sdef'"
check "build-app.sh installs the sdef into Resources" grep -q 'Ashdash.sdef' "$PACKAGING_DIR/build-app.sh"
check "executable name" equals "$(plutil -extract CFBundleExecutable raw -o - "$plist")" "ashdash"
check "entitlements valid" plutil -lint "$PACKAGING_DIR/entitlements.plist"

# --- icon (needs the Swift toolchain that ships with the Xcode command line tools)
if command -v swift >/dev/null 2>&1 && command -v iconutil >/dev/null 2>&1; then
  check "icon renders to .icns" "$PACKAGING_DIR/icon/make-icns.sh" "$work/AppIcon.icns"
  check "icns magic" bash -c "[ \"\$(head -c 4 '$work/AppIcon.icns')\" = icns ]"
else
  printf 'skip icon (swift/iconutil missing)\n'
fi

# --- Homebrew cask
printf 'not really a dmg' >"$work/Ashdash-1.2.3.dmg"
sha="$(shasum -a 256 "$work/Ashdash-1.2.3.dmg" | cut -d' ' -f1)"
"$PACKAGING_DIR/homebrew/render-cask.sh" --dmg "$work/Ashdash-1.2.3.dmg" --repo me/ashdash --output "$work/ashdash.rb" 2>/dev/null
check "cask has version" grep -q 'version "1.2.3"' "$work/ashdash.rb"
check "cask has sha256" grep -q "sha256 \"$sha\"" "$work/ashdash.rb"
check "cask has repo url" grep -q 'github.com/me/ashdash/releases/download/v#{version}/Ashdash-#{version}.dmg' "$work/ashdash.rb"
check "cask has no placeholders" bash -c "! grep -q '@[A-Z]*@' '$work/ashdash.rb'"
check "cask rejects bad sha" bash -c "! '$PACKAGING_DIR/homebrew/render-cask.sh' --version 1.0.0 --sha256 xyz 2>/dev/null"
if command -v ruby >/dev/null 2>&1; then
  check "cask is valid ruby" ruby -c "$work/ashdash.rb"
fi

# --- notarize.sh is a no-op without credentials and rejects missing input
check "notarize skips without NOTARY_PROFILE" bash -c "unset NOTARY_PROFILE; '$PACKAGING_DIR/notarize.sh' '$work/Ashdash-1.2.3.dmg'"
check "notarize rejects a missing file" bash -c "! '$PACKAGING_DIR/notarize.sh' '$work/nope.dmg' 2>/dev/null"

if [ "$fail" -ne 0 ]; then
  die "packaging self-test failed"
fi
echo "selftest: OK"

#!/usr/bin/env bash
# Create a plain, background-less DMG: Ashdash.app plus an Applications symlink.
#
# Usage: make-dmg.sh [path/to/Ashdash.app]     (default: $OUT_DIR/Ashdash.app)
# Environment:
#   OUT_DIR            where the DMG and its .sha256 are written (default: <repo>/dist)
#   CODESIGN_IDENTITY  when set to a real identity the DMG itself is signed too
#
# Output: $OUT_DIR/Ashdash-<version>.dmg and Ashdash-<version>.dmg.sha256
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

need hdiutil
need codesign
need shasum

NAME="$(brand_const NAME)"
OUT_DIR="${OUT_DIR:-$REPO_ROOT/dist}"
APP="${1:-$OUT_DIR/$NAME.app}"
[ -d "$APP" ] || die "app bundle not found: $APP (run packaging/build-app.sh first)"

# The version comes from the bundle being packaged, not from the working tree.
VERSION="$(plutil -extract CFBundleShortVersionString raw -o - "$APP/Contents/Info.plist")"
DMG="$OUT_DIR/$NAME-$VERSION.dmg"
IDENTITY="$(signing_identity)"

stage="$(mktemp -d "${TMPDIR:-/tmp}/ashdash-dmg.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

log "staging $NAME.app"
# ditto keeps signatures, xattrs and symlinks intact (cp -R can drop them).
ditto "$APP" "$stage/$NAME.app"
ln -s /Applications "$stage/Applications"

mkdir -p "$OUT_DIR"
rm -f "$DMG"
log "hdiutil create $DMG"
hdiutil create -volname "$NAME $VERSION" -srcfolder "$stage" -fs HFS+ -format UDZO -imagekey zlib-level=9 \
  -ov "$DMG" >/dev/null

if is_adhoc_identity "$IDENTITY"; then
  warn "ad-hoc build: the DMG is left unsigned"
else
  log "codesign DMG"
  codesign --force --sign "$IDENTITY" --timestamp "$DMG"
  codesign --verify --verbose=2 "$DMG"
fi

# Verify the image is well-formed (checksums of the compressed data).
log "hdiutil verify"
hdiutil verify "$DMG" >/dev/null

(cd "$OUT_DIR" && shasum -a 256 "$(basename "$DMG")" >"$(basename "$DMG").sha256")
cat "$DMG.sha256" >&2
printf '%s\n' "$DMG"

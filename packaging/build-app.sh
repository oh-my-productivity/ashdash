#!/usr/bin/env bash
# Build a universal (arm64 + x86_64) release of Ashdash and assemble Ashdash.app.
#
# Environment (all optional):
#   CODESIGN_IDENTITY  signing identity; unset or "-" means ad-hoc signing
#   ARCHS              space separated: "arm64 x86_64" (default) | "arm64" | "x86_64" | "native"
#   OUT_DIR            output directory (default: <repo>/dist)
#   BUILD_NUMBER       CFBundleVersion (default: git commit count)
#   MIN_OS             LSMinimumSystemVersion / deployment target (default 12.0)
#   SKIP_ICON=1        use a placeholder icon file instead of rendering one (offline/self-test)
#
# Output: $OUT_DIR/Ashdash.app (signed with the hardened runtime and verified).
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

need cargo
need lipo
need codesign
need plutil
need rustc

NAME="$(brand_const NAME)"
SLUG="$(brand_const SLUG)"
BUNDLE_ID="$(brand_const BUNDLE_ID)"
URL_SCHEME="$(brand_const URL_SCHEME)"
VERSION="$(app_version)"
BUILD="$(build_number)"
MIN_OS="${MIN_OS:-12.0}"
IDENTITY="$(signing_identity)"
OUT_DIR="${OUT_DIR:-$REPO_ROOT/dist}"
ARCHS="${ARCHS:-arm64 x86_64}"
APP="$OUT_DIR/$NAME.app"

if [ "$ARCHS" = "native" ]; then
  case "$(uname -m)" in
    arm64) ARCHS="arm64" ;;
    *) ARCHS="x86_64" ;;
  esac
fi

triple_for() {
  case "$1" in
    arm64) printf 'aarch64-apple-darwin' ;;
    x86_64) printf 'x86_64-apple-darwin' ;;
    *) die "unknown architecture '$1' (expected arm64 or x86_64)" ;;
  esac
}

log "$NAME $VERSION (build $BUILD), bundle id $BUNDLE_ID, archs: $ARCHS"
if is_adhoc_identity "$IDENTITY"; then
  warn "CODESIGN_IDENTITY not set: signing ad-hoc (fine for local use, not distributable)"
fi

# 1. Compile each architecture and join them with lipo.
export MACOSX_DEPLOYMENT_TARGET="$MIN_OS"
export CARGO_PROFILE_RELEASE_LTO="${CARGO_PROFILE_RELEASE_LTO:-thin}"
export CARGO_PROFILE_RELEASE_STRIP="${CARGO_PROFILE_RELEASE_STRIP:-symbols}"
installed_targets="$(rustup target list --installed 2>/dev/null || true)"
slices=()
for arch in $ARCHS; do
  triple="$(triple_for "$arch")"
  if [ -n "$installed_targets" ] && ! printf '%s\n' "$installed_targets" | grep -qx "$triple"; then
    die "rust target $triple is not installed (run: rustup target add $triple)"
  fi
  log "cargo build --release --target $triple"
  (cd "$REPO_ROOT" && cargo build --release --locked -p ashdash --target "$triple")
  slices+=("$REPO_ROOT/target/$triple/release/$SLUG")
done

work="$(mktemp -d "${TMPDIR:-/tmp}/ashdash-build.XXXXXX")"
trap 'rm -rf "$work"' EXIT
binary="$work/$SLUG"
if [ "${#slices[@]}" -eq 1 ]; then
  cp "${slices[0]}" "$binary"
else
  log "lipo -create (${#slices[@]} slices)"
  lipo -create -output "$binary" "${slices[@]}"
fi
lipo -info "$binary" >&2

# 2. Icon.
if [ "${SKIP_ICON:-0}" = "1" ]; then
  warn "SKIP_ICON=1: bundling a placeholder icon file"
  printf 'placeholder' >"$work/AppIcon.icns"
else
  "$PACKAGING_DIR/icon/make-icns.sh" "$work/AppIcon.icns"
fi

# 3. Assemble the bundle (always from scratch so stale files can never ship).
log "assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
install -m 755 "$binary" "$APP/Contents/MacOS/$SLUG"
install -m 644 "$work/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
printf 'APPL????' >"$APP/Contents/PkgInfo"
render_template "$REPO_ROOT/crates/app/resources/Info.plist.template" \
  "EXECUTABLE=$SLUG" "BUNDLE_ID=$BUNDLE_ID" "NAME=$NAME" "VERSION=$VERSION" "BUILD=$BUILD" \
  "MIN_OS=$MIN_OS" "URL_SCHEME=$URL_SCHEME" "YEAR=$(date +%Y)" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >&2

# 4. Sign with the hardened runtime.
sign_args=(--force --sign "$IDENTITY" --options runtime --entitlements "$PACKAGING_DIR/entitlements.plist"
  --identifier "$BUNDLE_ID")
if is_adhoc_identity "$IDENTITY"; then
  sign_args+=(--timestamp=none)
else
  sign_args+=(--timestamp)
fi
log "codesign (${IDENTITY/#-/ad-hoc})"
codesign "${sign_args[@]}" "$APP"

# 5. Verify.
log "codesign --verify --deep --strict"
codesign --verify --deep --strict --verbose=2 "$APP"
codesign --display --verbose=2 "$APP" 2>&1 | sed -n '/^Identifier/p;/^Format/p;/^CodeDirectory/p;/^Signature/p;/^TeamIdentifier/p' >&2
codesign --display --entitlements - "$APP" >/dev/null 2>&1 || warn "could not read back entitlements"

# spctl is Gatekeeper's verdict. Ad-hoc and un-notarized builds are rejected by design;
# that is informational, not a build failure.
if spctl_out="$(spctl --assess --type execute --verbose=2 "$APP" 2>&1)"; then
  printf 'spctl: %s\n' "$spctl_out" >&2
else
  printf 'spctl: %s\n' "$spctl_out" >&2
  if is_adhoc_identity "$IDENTITY"; then
    warn "spctl rejected the bundle - expected for an ad-hoc signature (not notarized)"
  else
    warn "spctl rejected the bundle - expected until it is notarized (packaging/notarize.sh)"
  fi
fi

printf '%s\n' "$APP"

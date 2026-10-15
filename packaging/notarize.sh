#!/usr/bin/env bash
# Notarize and staple a .app, .dmg or .pkg with Apple's notary service.
#
# Usage: notarize.sh <Ashdash.app | Ashdash-x.y.z.dmg>
# Environment:
#   NOTARY_PROFILE  keychain profile created once with
#                     xcrun notarytool store-credentials <profile> --apple-id ... --team-id ... --password ...
#                   When unset this script prints a notice and exits 0 (nothing is submitted).
#
# An .app is zipped (ditto) for upload and stapled in place; a .dmg is uploaded as-is.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

[ $# -eq 1 ] || die "usage: notarize.sh <Ashdash.app|Ashdash.dmg>"
target="$1"
[ -e "$target" ] || die "not found: $target"

if [ -z "${NOTARY_PROFILE:-}" ]; then
  log "NOTARY_PROFILE not set: skipping notarization of $target"
  exit 0
fi

need xcrun
need codesign
need plutil

# Apple rejects ad-hoc and non-hardened signatures; fail early with a clear message.
if [ -d "$target" ]; then
  sig="$(codesign --display --verbose=2 "$target" 2>&1 || true)"
  if printf '%s\n' "$sig" | grep -q 'Signature=adhoc'; then
    die "$target is signed ad-hoc; set CODESIGN_IDENTITY to a Developer ID Application identity and rebuild"
  fi
  if ! printf '%s\n' "$sig" | grep -q 'flags=.*runtime'; then
    die "$target is not signed with the hardened runtime"
  fi
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/ashdash-notary.XXXXXX")"
trap 'rm -rf "$work"' EXIT

upload="$target"
if [ -d "$target" ]; then
  upload="$work/$(basename "$target").zip"
  log "zipping $target for upload"
  ditto -c -k --keepParent "$target" "$upload"
fi

log "notarytool submit (this waits for Apple, usually a few minutes)"
# notarytool exits non-zero on transport problems; the verdict is in the JSON.
xcrun notarytool submit "$upload" --keychain-profile "$NOTARY_PROFILE" --wait --output-format json \
  >"$work/result.json" || true
status="$(plutil -extract status raw -o - "$work/result.json" 2>/dev/null || true)"
id="$(plutil -extract id raw -o - "$work/result.json" 2>/dev/null || true)"

if [ "$status" != "Accepted" ]; then
  warn "notarization status: ${status:-unknown} (submission id: ${id:-none})"
  if [ -n "$id" ]; then
    xcrun notarytool log "$id" --keychain-profile "$NOTARY_PROFILE" >&2 || true
  fi
  die "notarization failed"
fi
log "notarization accepted (submission $id)"

log "stapler staple"
xcrun stapler staple "$target"
xcrun stapler validate "$target"
if [ -d "$target" ]; then
  spctl --assess --type execute --verbose=2 "$target" >&2 || warn "spctl still rejects $target"
else
  spctl --assess --type open --context context:primary-signature --verbose=2 "$target" >&2 ||
    warn "spctl still rejects $target"
fi

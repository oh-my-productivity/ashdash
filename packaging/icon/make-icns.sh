#!/usr/bin/env bash
# Usage: make-icns.sh <output.icns>
# Renders the icon with CoreGraphics (make-icon.swift) and packs it with iconutil.
set -euo pipefail
# shellcheck source=../lib.sh
. "$(dirname "$0")/../lib.sh"

[ $# -eq 1 ] || die "usage: make-icns.sh <output.icns>"
out="$1"
need swift
need iconutil

work="$(mktemp -d "${TMPDIR:-/tmp}/ashdash-icon.XXXXXX")"
trap 'rm -rf "$work"' EXIT

swift "$PACKAGING_DIR/icon/make-icon.swift" "$work/AppIcon.iconset"
mkdir -p "$(dirname "$out")"
iconutil --convert icns --output "$out" "$work/AppIcon.iconset"
log "icon written to $out"

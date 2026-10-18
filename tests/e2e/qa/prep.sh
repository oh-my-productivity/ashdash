#!/usr/bin/env bash
# Prepares a CI runner for checklist.sh / soak.sh: TCC grants for the test tools (none names Ashdash), an ad-hoc ARCHS=native bundle built from this
# checkout and installed to /Applications without quarantine. ON CI (tests/e2e/ci/run-checklist.sh calls it):  bash tests/e2e/qa/prep.sh
set -uo pipefail
cd "$(dirname "$0")/../../.." || exit 1
# shellcheck source=lib.sh
. tests/e2e/qa/lib.sh
[ "${E2E_AUTO_TCC:-1}" = 1 ] && bash tests/e2e/guest/tcc-grant.sh >"$QA_ART/tcc-grant.txt" 2>&1
DIST=$HOME/qa-dist
OUT_DIR=$DIST ARCHS=native packaging/build-app.sh >"$QA_ART/build-app.log" 2>&1 || { tail -5 "$QA_ART/build-app.log"; exit 1; }
pkill -f "Ashdash.app/Contents/MacOS/ashdash" 2>/dev/null
sudo -n rm -rf /Applications/Ashdash.app && sudo -n ditto "$DIST/Ashdash.app" /Applications/Ashdash.app && sudo -n chown -R "$(id -un)" /Applications/Ashdash.app || exit 1
xattr -dr com.apple.quarantine /Applications/Ashdash.app 2>/dev/null
echo "prepared: $(plutil -extract CFBundleShortVersionString raw /Applications/Ashdash.app/Contents/Info.plist) at $(date +%H:%M:%S)"

#!/bin/bash
# Runs on a GitHub-hosted macOS runner (the e2e workflows call it through tests/e2e/ci/*): grants the test runners Accessibility, Post-Event,
# Screen Recording and Automation (System Events / Finder) by writing the TCC databases directly.
#
# This works because a hosted runner is a disposable Apple virtual machine with SIP *disabled* (see `csrutil status`) and passwordless sudo.
# It refuses to run anywhere else (see the guards below); in particular it never runs on a developer's Mac: end-to-end tests run on CI only
# (docs/TESTING.md).
# Clients: com.apple.Terminal (what tests/e2e/guest/gui-exec uses), com.apple.sshd-session (macOS 15+) and /usr/libexec/sshd-keygen-wrapper
# (older macOS), and /usr/bin/osascript.
set -euo pipefail
say() { printf '[tcc-grant] %s\n' "$*"; }
SYS="/Library/Application Support/com.apple.TCC/TCC.db"
USR="$HOME/Library/Application Support/com.apple.TCC/TCC.db"

# --- Guards. Everything here runs BEFORE the first write: this script grants permanent Accessibility / Screen Recording / Automation
# rights and restarts tccd, which must never happen on a developer's own Mac that merely has SIP turned off.
# 1. Only on a hosted CI runner: GITHUB_ACTIONS=true and RUNNER_ENVIRONMENT=github-hosted (GitHub sets both and a self-hosted runner says
#    "self-hosted"), the account "runner", and the hardware model "VirtualMac<N>,<M>" that Apple's virtualization framework reports (a
#    physical Mac says e.g. "Mac15,6"). All of them must agree, so exporting the CI variables on a developer's Mac changes nothing.
if [ "${GITHUB_ACTIONS:-}" != true ] || [ "${RUNNER_ENVIRONMENT:-}" != github-hosted ]; then
  say "refusing: this runs only on a GitHub-hosted runner (GITHUB_ACTIONS=true, RUNNER_ENVIRONMENT=github-hosted); end-to-end tests run on CI only (docs/TESTING.md)."; exit 1
fi
model=$(sysctl -n hw.model 2>/dev/null || true)
case $model in
  VirtualMac*) ;;
  *) say "refusing: this is not a virtual machine (hw.model='${model:-unknown}', expected VirtualMac*)."; exit 1 ;;
esac
[ "$(id -un)" = runner ] || { say "refusing: not the hosted-runner account (user '$(id -un)', expected 'runner')."; exit 1; }
say "GitHub-hosted runner (disposable VM): continuing"
# 2. SIP must be fully off; a partial configuration prints "enabled" for some parts and is not enough.
if ! csrutil status 2>&1 | grep -q 'status: disabled'; then
  say "SIP is not fully disabled: TCC.db is not writable."; exit 1
fi
# 3. Tools the script needs, before anything is written (a missing one used to leave a half-done grant).
if ! sudo -n true 2>/dev/null; then say "refusing: 'sudo -n' does not work (passwordless sudo is needed for the system TCC.db)."; exit 1; fi
for tool in sqlite3 python3; do
  command -v "$tool" >/dev/null 2>&1 || { say "refusing: '$tool' is not installed."; exit 1; }
done
python3 -c 'import plistlib' >/dev/null 2>&1 || { say "refusing: python3 does not work (needs plistlib)."; exit 1; }

# grant <db> <sudo|""> <service> <client> <client_type> [<indirect id>]
grant() {
  local db=$1 sudo=$2 service=$3 client=$4 ctype=$5 ind=${6:-UNUSED} indtype=0
  [ "$ind" = UNUSED ] && indtype=NULL
  local sql="insert or replace into access(service,client,client_type,auth_value,auth_reason,auth_version,csreq,policy_id,indirect_object_identifier_type,indirect_object_identifier,indirect_object_code_identity,flags,last_modified) values('$service','$client',$ctype,2,4,1,NULL,NULL,$indtype,'$ind',NULL,0,CAST(strftime('%s','now') AS INTEGER));"
  if [ -n "$sudo" ]; then sudo -n sqlite3 -cmd ".timeout 15000" "$db" "$sql"; else sqlite3 -cmd ".timeout 15000" "$db" "$sql"; fi
}

for c in "com.apple.Terminal:0" "com.apple.sshd-session:0" "/usr/libexec/sshd-keygen-wrapper:1" "/usr/bin/osascript:1"; do
  client=${c%:*} ctype=${c##*:}
  for svc in kTCCServiceAccessibility kTCCServicePostEvent kTCCServiceScreenCapture; do
    grant "$SYS" sudo "$svc" "$client" "$ctype"
  done
  for target in com.apple.systemevents com.apple.finder; do
    grant "$USR" "" kTCCServiceAppleEvents "$client" "$ctype" "$target"
  done
  say "granted $client"
done
# macOS 15 asks a second, non-TCC question the first time (and then every 30 days) an app records the screen: "<app> is requesting to
# bypass the system private window picker...". Its answers live in a plist read by replayd; seed it with a hint date far in the future.
python3 - <<'PY'
import datetime, os, plistlib
d = os.path.expanduser("~/Library/Group Containers/group.com.apple.replayd")
os.makedirs(d, exist_ok=True)
path = os.path.join(d, "ScreenCaptureApprovals.plist")
try:
    with open(path, "rb") as f:
        data = plistlib.load(f)
except (OSError, plistlib.InvalidFileException):
    data = {}
now = datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None)
far = now + datetime.timedelta(days=3650)
for client in ("com.apple.Terminal", "com.apple.sshd-session", "/usr/libexec/sshd-keygen-wrapper"):
    data[client] = {
        "kScreenCaptureAlertableUsageCount": 1,
        "kScreenCaptureApprovalLastAlerted": now,
        "kScreenCaptureApprovalLastUsed": now,
        "kScreenCapturePrivacyHintDate": far,
        "kScreenCapturePrivacyHintPolicy": 2592000,
    }
with open(path, "wb") as f:
    plistlib.dump(data, f)
PY
killall replayd 2>/dev/null || true
# tccd caches decisions: restart the system and the user daemon so the new rows apply without a reboot.
sudo -n killall tccd 2>/dev/null || true
killall tccd 2>/dev/null || true
sleep 2
say "done"

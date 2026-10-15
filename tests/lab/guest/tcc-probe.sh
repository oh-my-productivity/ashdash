#!/bin/bash
# shellcheck disable=SC2012
# Runs on the GUEST: experiment, "can the TCC grants be scripted?" Prints facts only; changes nothing permanent.
echo "SIP:        $(csrutil status 2>&1 | head -1)"
echo "user TCC:   $HOME/Library/Application Support/com.apple.TCC/TCC.db"
echo "system TCC: /Library/Application Support/com.apple.TCC/TCC.db"
for db in "$HOME/Library/Application Support/com.apple.TCC/TCC.db" "/Library/Application Support/com.apple.TCC/TCC.db"; do
  echo "--- $db"
  ls -lO "$db" 2>&1 | head -2
  echo "read:   $(sqlite3 "$db" 'select count(*) from access;' 2>&1 | head -1)"
  echo "insert: $(sudo -n sqlite3 "$db" "insert or ignore into access(service,client,client_type,auth_value,auth_reason,auth_version,flags) values('kTCCServiceAccessibility','com.apple.Terminal',0,2,4,1,0);" 2>&1 | head -1) (rc=$?)"
  echo "count after: $(sudo -n sqlite3 "$db" "select count(*) from access where client='com.apple.Terminal';" 2>&1 | head -1)"
done
echo "--- profiles / MDM"
profiles status -type enrollment 2>&1 | head -3
echo "--- tccutil (can only reset, never grant)"
tccutil help 2>&1 | head -2

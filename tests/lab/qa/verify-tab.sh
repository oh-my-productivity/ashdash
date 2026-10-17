#!/usr/bin/env bash
# 3.3.3b re-check on the bundle built from this checkout: Tab on an application row completes the field to the app's name.
# INSIDE a lab VM:  bash tests/lab/qa/prep.sh && bash tests/lab/qa/verify-tab.sh
set -uo pipefail
cd "$(dirname "$0")/../../.." || exit 1
# shellcheck source=checklist.sh
. tests/lab/qa/checklist.sh
reseed; start_app
show || { result 3.3.3b FAIL "launcher did not show"; exit 1; }
clear_field; type_text saf; sleep 1.2
before=$(look tab-before); key_code $KC_TAB; sleep 1; after=$(look tab-after)
if has "$after" '^\s*Safari\s*\|' && ! has "$after" '^\s*saf\s*\|'; then result 3.3.3b PASS "Tab on the Safari row completes the field to 'Safari' (before: $before; after: $after) [fixed in providers/apps.rs, test application_rows_tab_complete_to_their_name]"
else result 3.3.3b FAIL "after Tab the field is not 'Safari': $after"; fi
hide_; stop_app

#!/usr/bin/env bash
# Wave 5 lab screenshot: the field showed its placeholder ("Ashdash") while results for "saf" were listed. INSIDE a lab VM after prep.sh:
#   bash tests/lab/qa/repro-placeholder.sh [trials=40]
# Types 'saf' after the hotkey in several ways (fresh show; Esc-then-retype; Cmd+A/Delete-then-retype; ashdash://search; `ashdash show`)
# and takes a window screenshot at several delays; a trial is a HIT when the OCR of the window has the placeholder as its first line and a
# result row ('Safari') below it. Results (id REPRO-PLACEHOLDER) go to $QA_RESULTS.
# shellcheck disable=SC2015
set -uo pipefail
cd "$(dirname "$0")/../../.." || exit 1
# shellcheck source=checklist.sh
. tests/lab/qa/checklist.sh
N=${1:-40}
reseed; start_app
hits=0; total=0; detail=""
trial() { # trial <label> <delay> : state is prepared by the caller, the text typed
  local o; sleep "$2"; o=$(look "ph-$1" 2>/dev/null); total=$((total+1))
  if echo "$o" | head -c 400 | grep -qiE '^\s*Ashdash\s*\|' && has "$o" 'Safari'; then hits=$((hits+1)); detail="$detail [$1 @$2: $o]"; cp "$QA_ART/ph-$1.png" "$QA_ART/ph-HIT-$total.png"; fi
}
for i in $(seq 1 "$N"); do
  d=$(printf '%s\n' 0.15 0.3 0.6 1.2 | sed -n "$(( i % 4 + 1 ))p")
  case $(( i % 5 )) in
    0) hide_; show; type_text saf; trial fresh "$d" ;;
    1) show; clear_field; type_text saf; trial retype "$d" ;;
    2) show; type_text saf; sleep 1; key_code $KC_ESC; sleep 0.2; type_text saf; trial esc-retype "$d" ;;
    3) hide_; wd 10 /usr/bin/open "ashdash://search?query=saf"; trial url "$d" ;;
    4) hide_; HOME=$QAH wd 10 "$BIN" show >/dev/null; type_text saf; trial cli-show "$d" ;;
  esac
done
hide_
if [ "$hits" = 0 ]; then result REPRO-PLACEHOLDER PASS "not reproduced in $total trials (fresh show, retype, Esc+retype, ashdash://search, CLI show; delays 0.15-1.2 s)"
else result REPRO-PLACEHOLDER FAIL "reproduced $hits of $total: $detail"; fi

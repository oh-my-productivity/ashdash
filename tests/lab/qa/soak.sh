#!/usr/bin/env bash
# QA item 4: soak. INSIDE a lab VM, from the synced worktree:
#   nohup bash tests/lab/qa/soak.sh [seconds=7200] [interval=30] >/dev/null 2>&1 &     (detached: it outlives the ssh session)
# Every INTERVAL seconds: press the hotkey (the launcher must appear), type 'saf', wait, hide with Esc. Every 5 minutes: a sample line in
# $SOAK_DIR/samples.tsv (time, pid, RSS KB, footprint MB, cputime, threads, open files, windows, crash reports, misses so far).
# At the end: the hotkey must still work, crash reports are listed, the verdict goes to $SOAK_DIR/result.txt.
# shellcheck disable=SC2015
set -uo pipefail
cd "$(dirname "$0")/../../.." || exit 1
SOAK_DIR=${SOAK_DIR:-$HOME/qa-soak}; mkdir -p "$SOAK_DIR"
export QA_ART=$SOAK_DIR QA_RESULTS=$SOAK_DIR/results.tsv
# shellcheck source=checklist.sh
. tests/lab/qa/checklist.sh
DURATION=${1:-7200} INTERVAL=${2:-30} SAMPLE=300
LOG=$SOAK_DIR/soak.log
log() { printf '%s %s\n' "$(date +%H:%M:%S)" "$*" >>"$LOG"; }

reseed; start_app
pid=$(app_pid); [ -n "$pid" ] || { log "app did not start"; echo "FAIL: app did not start" >"$SOAK_DIR/result.txt"; exit 1; }
t_start=$(date +%s); next_sample=$t_start; cycles=0; misses=0
printf 'time\tpid\trss_kb\tfootprint_mb\tcputime\tthreads\topen_files\twindows\tcrash_reports\tmisses\tcycles\n' >"$SOAK_DIR/samples.tsv"
sample() {
  local fp rss ct th fd cr
  fp=$(footprint -p "$pid" 2>/dev/null | grep -iE 'Footprint:' | head -1 | sed -E 's/.*Footprint: *([0-9.]+) *([KMG]B).*/\1 \2/')
  rss=$(ps -o rss= -p "$pid" | tr -d ' '); ct=$(ps -o cputime= -p "$pid" | tr -d ' ')
  th=$(ps -M -p "$pid" 2>/dev/null | tail -n +2 | wc -l | tr -d ' '); fd=$(lsof -p "$pid" 2>/dev/null | wc -l | tr -d ' ')
  cr=$(( $(ls "$DATA/crashes" 2>/dev/null | grep -c '^crash-') + $(ls "$HOME/Library/Logs/DiagnosticReports" 2>/dev/null | grep -ci ashdash) ))
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$(date +%H:%M:%S)" "$pid" "$rss" "$fp" "$ct" "$th" "$fd" "$(nwin)" "$cr" "$misses" "$cycles" >>"$SOAK_DIR/samples.tsv"
}
log "soak start: pid $pid, $DURATION s, every $INTERVAL s"
while [ $(( $(date +%s) - t_start )) -lt "$DURATION" ]; do
  c0=$(date +%s)
  if ! kill -0 "$pid" 2>/dev/null; then log "PROCESS GONE (pid $pid) after $cycles cycles"; misses=$((misses+1000)); break; fi
  hide_
  hotkey; ok=0; for _ in 1 2 3 4 5 6 7 8; do sleep 0.25; visible && { ok=1; break; }; done
  if [ $ok = 1 ]; then type_text "saf"; sleep 1.5; key_code $KC_ESC; sleep 0.5; visible && { hotkey; sleep 0.5; }
  else misses=$((misses+1)); log "MISS: hotkey did not show the launcher (cycle $cycles)"; hotkey; sleep 1; fi
  cycles=$((cycles+1))
  if [ "$(date +%s)" -ge "$next_sample" ]; then sleep 8; sample; next_sample=$(( $(date +%s) + SAMPLE )); log "sample: $(tail -1 "$SOAK_DIR/samples.tsv" | cut -f3,4,5)"; fi
  left=$(( INTERVAL - ( $(date +%s) - c0 ) )); [ "$left" -gt 0 ] && sleep "$left"
done
# end state
sample
hide_; hotkey; ok_end=0; for _ in 1 2 3 4 5 6 7 8; do sleep 0.25; visible && { ok_end=1; break; }; done; shot soak-end >/dev/null 2>&1; hide_
crashes=$(ls "$DATA/crashes" 2>/dev/null | grep '^crash-' | tr '\n' ' '); ips=$(ls "$HOME/Library/Logs/DiagnosticReports" 2>/dev/null | grep -i ashdash | tr '\n' ' ')
{
  echo "duration_s=$(( $(date +%s) - t_start )) cycles=$cycles misses=$misses hotkey_works_at_end=$ok_end process_alive=$(kill -0 "$pid" 2>/dev/null && echo yes || echo no)"
  echo "crash reports (app): ${crashes:-none}; macOS crash logs: ${ips:-none}"
  echo "first sample: $(sed -n 2p "$SOAK_DIR/samples.tsv" | cut -f3-5); last sample: $(tail -1 "$SOAK_DIR/samples.tsv" | cut -f3-5)"
} >"$SOAK_DIR/result.txt"
log "soak done"
[ "$ok_end" = 1 ] && [ "$misses" = 0 ] && [ -z "$crashes$ips" ] && result SOAK PASS "$(head -1 "$SOAK_DIR/result.txt")" || result SOAK FAIL "$(tr '\n' ' ' <"$SOAK_DIR/result.txt")"

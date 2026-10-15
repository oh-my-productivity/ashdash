#!/usr/bin/env bash
# Shared helpers for the Ashdash test lab. Sourced by the lab-* scripts; never executed directly.
# shellcheck shell=bash

# Agents often run with a throw-away HOME; tart images, the lab key and the ssh state must still
# be found, so resolve the *real* home from the account database instead of $HOME.
LAB_REAL_HOME=${LAB_REAL_HOME:-$(eval echo "~$(id -un)")}
LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
LAB_REPO_ROOT=$(cd "$LAB_DIR/../.." && pwd)

: "${TART_BIN:=$LAB_REAL_HOME/.local/bin/tart}"
if [ ! -x "$TART_BIN" ]; then TART_BIN=$(command -v tart || true); fi
export TART_HOME=${TART_HOME:-$LAB_REAL_HOME/.tart}
# A clone must never trigger tart's automatic pruning of the (expensive) base image cache.
export TART_NO_AUTO_PRUNE=1

LAB_BASE_IMAGE=${LAB_BASE_IMAGE:-ghcr.io/cirruslabs/macos-sequoia-base:latest}
LAB_GOLDEN=${LAB_GOLDEN:-ashdash-golden}
LAB_PREFIX=ashdash-run-
LAB_USER=${LAB_USER:-admin}
LAB_PASSWORD=${LAB_PASSWORD:-admin}
LAB_MAX_VM_AGE=${LAB_MAX_VM_AGE:-1800}            # seconds; watchdog deletes ashdash-run-* older than this
LAB_QUEUE_TIMEOUT=${LAB_QUEUE_TIMEOUT:-2700}      # seconds a job may wait for a free slot
LAB_SLOTS=${LAB_SLOTS:-2}                         # Apple: max 2 concurrent macOS VMs per Apple Silicon host
# /tmp rather than $TMPDIR: every agent must see the same lock files whatever TMPDIR it runs with.
LAB_RUN_DIR=${LAB_RUN_DIR:-/tmp/ashdash-lab}
LAB_STATE_DIR=${LAB_STATE_DIR:-$LAB_REAL_HOME/.local/state/ashdash-lab}
LAB_KEY=${LAB_KEY:-$LAB_STATE_DIR/id_ed25519}
LAB_REMOTE_ROOT=${LAB_REMOTE_ROOT:-ashdash-work}   # relative to the guest user's home

mkdir -p "$LAB_RUN_DIR" "$LAB_RUN_DIR/logs" "$LAB_RUN_DIR/pids"
chmod 700 "$LAB_RUN_DIR" 2>/dev/null || true

lab_log() { printf 'lab: %s\n' "$*" >&2; }
lab_die() { printf 'lab: ERROR: %s\n' "$*" >&2; exit 1; }

lab_need_tart() {
  [ -n "${TART_BIN:-}" ] && [ -x "$TART_BIN" ] || lab_die "tart not found (expected $LAB_REAL_HOME/.local/bin/tart)"
}

# Only ever touch disposable run VMs by name: a typo must never delete the golden image.
lab_check_run_name() {
  case "$1" in
    "${LAB_PREFIX}"?*) ;;
    *) lab_die "refusing '$1': lab VMs are named ${LAB_PREFIX}<id>" ;;
  esac
  case "$1" in
    *[!A-Za-z0-9._-]*) lab_die "invalid VM name '$1'" ;;
  esac
}

lab_vm_exists() { "$TART_BIN" list --source local --quiet 2>/dev/null | grep -Fxq -- "$1"; }

lab_vm_running() {
  "$TART_BIN" list --source local --format json 2>/dev/null |
    python3 -c '
import json, sys
name = sys.argv[1]
for vm in json.load(sys.stdin):
    if vm.get("Name") == name and (vm.get("Running") or vm.get("State") == "running"):
        sys.exit(0)
sys.exit(1)' "$1"
}

# Seconds since the VM directory was created (the clone time).
lab_vm_age() {
  local dir="$TART_HOME/vms/$1" born
  [ -d "$dir" ] || { echo 0; return; }
  born=$(stat -f %B "$dir" 2>/dev/null || stat -c %W "$dir" 2>/dev/null || echo 0)
  if [ "$born" -le 0 ] 2>/dev/null; then born=$(stat -f %m "$dir" 2>/dev/null || stat -c %Y "$dir"); fi
  echo $(( $(date +%s) - born ))
}

lab_ssh_opts() {
  printf '%s\n' -F /dev/null -i "$LAB_KEY" -o IdentitiesOnly=yes -o BatchMode=yes \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR \
    -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=8
}

# lab_ssh <ip> <remote command string>
lab_ssh() {
  local ip=$1; shift
  local opts=()
  while IFS= read -r line; do opts+=("$line"); done < <(lab_ssh_opts)
  ssh "${opts[@]}" "$LAB_USER@$ip" "$@"
}

lab_vm_ip() { "$TART_BIN" ip "$1" --wait "${2:-0}" 2>/dev/null; }

# Python helper for the slot semaphore (macOS ships no flock(1)).
lab_slot() { python3 "$LAB_DIR/slot.py" --run-dir "$LAB_RUN_DIR" --slots "$LAB_SLOTS" "$@"; }

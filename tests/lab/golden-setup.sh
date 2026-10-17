#!/usr/bin/env bash
# golden-setup.sh <create|gui|verify|manual-steps|tcc-probe> [--force]
#
#   create        Build `ashdash-golden` from the cirruslabs base image (must already be pulled:
#                 `tart pull $LAB_BASE_IMAGE`): clone, boot headless, install the lab ssh key (password
#                 login through an SSH_ASKPASS helper, no sshpass needed), install Xcode CLT + rust,
#                 disable sleep/lock, check auto-login, copy tests/lab/guest to ~/lab-guest, shut down.
#                 --force deletes an existing ashdash-golden first.
#   gui           Boot the golden image WITH a window so you can do the manual TCC grants (blocks until you
#                 close the window / the VM stops). Holds one of the two VM slots while open.
#   verify        Throw-away clone: report toolchain, sleep/lock/auto-login state and TCC state.
#   manual-steps  Print the manual steps again.
#   tcc-probe     Throw-away clone: try to grant Accessibility/Automation by script (see docs/LAB.md).
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

cmd=${1:-}; shift || true
force=0
for a in "$@"; do [ "$a" = "--force" ] && force=1; done
lab_need_tart

manual_steps() {
  cat >&2 <<'STEPS'

================================ MANUAL STEPS (cannot be scripted) ================================
Run:   tests/lab/golden-setup.sh gui        (opens the VM window; log in is automatic, password: admin)

In the VM, System Settings > Privacy & Security:
  1. Accessibility   -> add/enable  Terminal   (and /usr/libexec/sshd-keygen-wrapper: press + , Cmd+Shift+G)
  2. Automation      -> run once in Terminal:  osascript -e 'tell application "System Events" to keystroke ""'
                        and answer "OK" to the "Terminal wants to control System Events" prompt.
  3. Screen & System Audio Recording -> enable Terminal (needed for screencapture to include windows).
  4. Terminal > Settings > Profiles: "When the shell exits: Close if the shell exited cleanly" (optional).
  5. Quit the VM window (Apple menu > Shut Down). Then:  tests/lab/golden-setup.sh verify

Why: osascript/System Events/screencapture are attributed to the responsible app. tests/lab/guest/gui-exec runs
jobs inside the GUI session via Terminal.app so one grant covers every test. See docs/LAB.md.
===================================================================================================
STEPS
}

askpass_file=""
cleanup() { [ -n "$askpass_file" ] && rm -f "$askpass_file"; }
trap cleanup EXIT

# ssh with the password supplied by an SSH_ASKPASS helper (OpenSSH >= 8.4: SSH_ASKPASS_REQUIRE=force).
pw_ssh() {
  if [ -z "$askpass_file" ]; then
    askpass_file=$(umask 077; mktemp "${TMPDIR:-/tmp}/lab-askpass.XXXXXX")
    printf '#!/bin/sh\nprintf %%s "%s"\n' "$LAB_PASSWORD" >"$askpass_file"
    chmod 700 "$askpass_file"
  fi
  SSH_ASKPASS="$askpass_file" SSH_ASKPASS_REQUIRE=force DISPLAY=lab:0 \
    ssh -F /dev/null -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR \
        -o PubkeyAuthentication=no -o PreferredAuthentications=password,keyboard-interactive \
        -o NumberOfPasswordPrompts=1 -o ConnectTimeout=10 "$@"
}

boot_headless() { # boot_headless <vm> ; prints ip
  local vm=$1 ip
  nohup "$TART_BIN" run --no-graphics --no-audio "$vm" >"$LAB_RUN_DIR/logs/$vm.log" 2>&1 &
  echo $! >"$LAB_RUN_DIR/pids/$vm.pid"
  ip=$(lab_vm_ip "$vm" 240) || lab_die "$vm got no IP"
  echo "$ip"
}

stop_vm() {
  "$TART_BIN" stop "$1" --timeout 30 >/dev/null 2>&1 || true
  local pid; pid=$(cat "$LAB_RUN_DIR/pids/$1.pid" 2>/dev/null || true)
  [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
}

toolchain() {
  if [ -n "${LAB_RUST_TOOLCHAIN:-}" ]; then echo "$LAB_RUST_TOOLCHAIN"; return; fi
  if [ -f "$LAB_REPO_ROOT/rust-toolchain.toml" ]; then sed -n 's/^channel *= *"\(.*\)"/\1/p' "$LAB_REPO_ROOT/rust-toolchain.toml" | head -1; return; fi
  rustc --version 2>/dev/null | awk '{print $2}' || echo stable
}

case "$cmd" in
  create)
    lab_image_exists "$LAB_BASE_IMAGE" || lab_die "base image $LAB_BASE_IMAGE is not pulled yet ('tart list'); run: $TART_BIN pull $LAB_BASE_IMAGE"
    if lab_vm_exists "$LAB_GOLDEN"; then
      [ "$force" = 1 ] || lab_die "$LAB_GOLDEN exists; use --force to rebuild it"
      "$TART_BIN" stop "$LAB_GOLDEN" --timeout 10 >/dev/null 2>&1 || true
      "$TART_BIN" delete "$LAB_GOLDEN"
    fi
    lab_log "waiting for a VM slot"
    lab_slot acquire --name golden-setup --timeout "$LAB_QUEUE_TIMEOUT" --max-age 5400 --no-delete >/dev/null || lab_die "no slot"
    trap 'stop_vm "$LAB_GOLDEN"; lab_slot release --name golden-setup; cleanup' EXIT
    lab_log "cloning $LAB_BASE_IMAGE -> $LAB_GOLDEN"
    "$TART_BIN" clone "$LAB_BASE_IMAGE" "$LAB_GOLDEN"
    # A little more CPU/RAM than the default makes cargo builds in the VM tolerable; disk stays CoW-small.
    "$TART_BIN" set "$LAB_GOLDEN" --cpu "${LAB_VM_CPU:-4}" --memory "${LAB_VM_MEMORY_MB:-8192}"
    ip=$(boot_headless "$LAB_GOLDEN")
    lab_log "booted at $ip; waiting for sshd"
    for _ in $(seq 1 90); do pw_ssh "$LAB_USER@$ip" true 2>/dev/null && break; sleep 2; done
    pw_ssh "$LAB_USER@$ip" true || lab_die "password ssh to $LAB_USER@$ip failed (is Remote Login on in the image?)"
    mkdir -p "$LAB_STATE_DIR"; chmod 700 "$LAB_STATE_DIR"
    [ -f "$LAB_KEY" ] || ssh-keygen -q -t ed25519 -N '' -C ashdash-lab -f "$LAB_KEY"
    lab_log "installing ssh key"
    # shellcheck disable=SC2016  # expanded on the guest, on purpose
    pw_ssh "$LAB_USER@$ip" 'umask 077; mkdir -p ~/.ssh; touch ~/.ssh/authorized_keys; k=$(cat); grep -qxF "$k" ~/.ssh/authorized_keys || printf "%s\n" "$k" >> ~/.ssh/authorized_keys' <"$LAB_KEY.pub"
    lab_ssh "$ip" true || lab_die "key auth does not work after install"
    lab_log "provisioning (toolchain $(toolchain))"
    lab_ssh "$ip" "bash -s -- $(printf %q "$(toolchain)") $(printf %q "$LAB_PASSWORD")" <"$LAB_DIR/guest/provision.sh"
    lab_log "copying guest helpers to ~/lab-guest"
    ssh_opts=(); while IFS= read -r l; do ssh_opts+=("$l"); done < <(lab_ssh_opts)
    rsync -a --delete -e "ssh ${ssh_opts[*]}" "$LAB_DIR/guest/" "$LAB_USER@$ip:lab-guest/"
    lab_ssh "$ip" 'chmod +x ~/lab-guest/*'
    lab_log "shutting down"
    lab_ssh "$ip" 'sudo -n shutdown -h now' >/dev/null 2>&1 || true
    stop_vm "$LAB_GOLDEN"
    manual_steps
    lab_log "golden image built. Do the manual steps above, then run: tests/lab/golden-setup.sh verify"
    ;;
  gui)
    lab_vm_exists "$LAB_GOLDEN" || lab_die "$LAB_GOLDEN does not exist (run create)"
    lab_slot acquire --name golden-gui --timeout "$LAB_QUEUE_TIMEOUT" --max-age 5400 --no-delete >/dev/null || lab_die "no slot"
    trap 'lab_slot release --name golden-gui; cleanup' EXIT
    manual_steps
    lab_log "opening the VM window (close it / shut down the guest to finish)"
    "$TART_BIN" run "$LAB_GOLDEN"
    ;;
  verify)
    "$LAB_DIR/lab-with-vm" -- bash tests/lab/guest/check-golden.sh
    ;;
  tcc-probe)
    "$LAB_DIR/lab-with-vm" -- bash tests/lab/guest/tcc-probe.sh
    ;;
  manual-steps) manual_steps ;;
  *) sed -n '2,17p' "$0"; exit 2 ;;
esac

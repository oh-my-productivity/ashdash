# Isolated test lab (WP14)

Anything that shows a window, registers a global hotkey / URL handler / login item, sends keystrokes, takes a
screenshot or triggers a macOS permission prompt runs **in a throw-away macOS VM**, never on the user's desktop.
The lab is a thin layer of shell scripts around [Tart](https://tart.run) (Apple Virtualization.framework) in
`tests/lab/`. Design background: `docs/PLAN.md` section 8.

## Agent quick start

```sh
# One-shot: boot a fresh VM, sync this worktree, run a command inside it, delete the VM.
tests/lab/lab-with-vm -- uname -a
tests/lab/lab-with-vm --artifacts out/ -- bash tests/lab/smoke.sh --quick

# The full smoke job (build, scripts/check.sh, bundle, launch, hotkey, URL scheme, login item, quit):
tests/lab/lab-smoke                    # report + artifacts land in lab-out/<timestamp>/ (git-ignored)
tests/lab/lab-smoke --quick            # skip scripts/check.sh

# Several commands against one VM (e.g. build once, poke many times):
read -r vm ip < <(tests/lab/lab-up)    # prints "ashdash-run-<id> <ip>" on stdout, progress on stderr
tests/lab/lab-run "$vm" -- cargo build -p ashdash
tests/lab/lab-run "$vm" --no-sync --artifacts out/ -- bash tests/lab/smoke.sh --no-gui
tests/lab/lab-down "$vm"               # ALWAYS, also after failures (or use lab-with-vm, which cannot forget)
```

Rules:

- GUI / hotkey / URL-scheme / login-item / dialog tests go through the lab. The `/tmp/ashdash-desktop.lock`
  desktop mutex from the wave briefs is only for the rare check that really needs the host desktop.
- Inside the VM the command runs over ssh from the synced copy of your worktree (`~/ashdash-work/<worktree name>`),
  with `~/.cargo/bin` on `PATH`, and with `$LAB_ARTIFACTS` pointing at a fresh directory whose contents `--artifacts DIR`
  copies back to the host (also when the command fails). Screenshots are artifacts only; look at them, delete them, never commit them.
- ssh sessions are *not* part of the desktop session. To start something in the logged-in GUI session (windows, hotkeys,
  System Events keystrokes, `screencapture`) use `tests/lab/guest/gui-exec -- <cmd>` from inside the VM (see "TCC grants").
- Host-only tests (cargo unit tests, temp HOME) never need the lab or a slot.
- `lab-run`, `lab-down` only accept names of the form `ashdash-run-<id>`; the golden image cannot be modified or deleted by them.

## What is in `tests/lab/`

| File | Purpose |
|---|---|
| `lab-up [--id ID] [--image NAME]` | gc, wait for a slot, `tart clone ashdash-golden ashdash-run-<id>`, `tart run --no-graphics`, wait for an IP and for ssh. Idempotent for a given `--id`. Prints `<vm> <ip>`. |
| `lab-run <vm> [--artifacts DIR] [--no-sync] [--src DIR] [--env K=V] -- cmd...` | rsync the worktree in (honours `.gitignore`; skips `target/`, `.worktrees/`, `.git`; the guest's `target/` persists between runs), run `cmd` over ssh, stream output, propagate the exit code, copy artifacts out. |
| `lab-down <vm>...` | stop + delete + release the slot. Idempotent; fails loudly if the VM still exists. |
| `lab-with-vm [opts] -- cmd...` | up, run, down with a `trap` for EXIT/INT/TERM/HUP. Exit code is the command's. |
| `lab-smoke [--quick] [--no-gui]` | `lab-with-vm` running `smoke.sh`, artifacts in `lab-out/<timestamp>/`. |
| `lab-gc [--max-age S]` | watchdog: delete `ashdash-run-*` VMs older than 30 min (`LAB_MAX_VM_AGE`). `lab-up` runs it first. |
| `slot.py` | the two-slot semaphore (see below). `slot.py status` shows who holds what. |
| `golden-setup.sh create|gui|verify|manual-steps|tcc-probe` | build and maintain the golden image. |
| `smoke.sh` | the job that runs inside the VM. |
| `guest/` | helpers copied into the golden image (`~/lab-guest`) and used from the synced tree: `gui-exec`, `provision.sh`, `check-golden.sh`, `tcc-probe.sh`. |
| `test/run-tests.sh` | host-side tests of all of the above against a fake `tart` and fake `ssh` (no VM; ~100 s). |
| `lib.sh` | shared settings. Every knob is an environment variable (`LAB_*`, `TART_HOME`), see the top of the file. |

Name resolution does not depend on `$HOME`: agents run with throw-away HOMEs, so the scripts resolve the real home
(`~/.tart`, `~/.local/state/ashdash-lab`, `~/.local/bin/tart`) from the account database.

## The 2-VM limit and the slot queue

Apple's licence and the kernel allow **at most two macOS guests at the same time on an Apple Silicon host**. A third `tart run`
fails, so the lab serialises with two slots (`/tmp/ashdash-lab/slots/slot{0,1}.lock`; `/tmp`, not `$TMPDIR`, because agents run
with different `TMPDIR`s and must all see the same files).

- `lab-up` calls `slot.py acquire`: it polls (about once a second, printing who holds the slots every 30 s) until `flock` on one of the files
  succeeds, up to `LAB_QUEUE_TIMEOUT` (45 min), then exits non-zero. A detached *holder* process keeps the lock for the life of the VM.
  The queue is "whoever polls first", not strictly FIFO.
- The kernel releases a `flock` when its holder dies, so there are no stale lock files to clean. The holder also exits when the VM has gone
  away, when `lab-down` signals it, when the VM never appeared within 10 min (`lab-up` died), and after the maximum age (30 min), at which point it
  **stops and deletes its VM** itself. `lab-gc` is a second line of defence (VM age = clone time).
- Your own jobs count too: two `lab-with-vm` jobs of one agent fill both slots. A job that holds a VM for long blocks everybody; keep jobs short,
  build once with `lab-up` + `lab-run` when you need many steps.
- `golden-setup.sh create|gui` also take a slot. Do not start tart VMs by hand while the lab is in use.
- Not enforced by the lab: someone running `tart run` directly, or any other VM software, still counts against Apple's limit.

## Cost

(Measured on this host, Apple M3 Pro, 12 cores, 36 GB RAM, macOS 26.5.2; see "Measurements" at the end.)

- Disk: the pulled base image and the golden image are full copies (tens of GB each); every `ashdash-run-*` clone is APFS copy-on-write and
  only grows by what the job writes, mainly the guest `target/` directory (a few GB after a first debug build). Clones are deleted by `lab-down`.
- Time: clone is instant; boot to ssh takes a short while (measured below); the first `cargo build` in a fresh clone compiles every
  dependency, later `lab-run` calls in the *same* VM are incremental. A one-shot `lab-smoke` therefore pays a full build each time
  (use `--quick`, or rebuild the golden image with a warm `~/.cargo/registry`, see below).
- RAM: 8 GB and 4 CPUs per VM by default (`LAB_VM_MEMORY_MB`, `LAB_VM_CPU` at golden-build time), 16 GB for the two slots.

## TCC grants (Accessibility, Automation, Screen Recording)

macOS asks for these interactively and attributes them to the *responsible* app. In the golden image that is **Terminal.app**:
`tests/lab/guest/gui-exec -- <cmd>` opens a `.command` file with Terminal inside the logged-in GUI session, so one set of grants covers every
test. Without the grants the lab still builds, runs tests, starts the app in the GUI session and drives it through its CLI/IPC/URL scheme;
only System Events keystrokes (the global hotkey press) and full-desktop screenshots are skipped, and `smoke.sh` records that as `SKIP` with the reason.

### Manual steps (once per golden image; cannot be scripted, see the result of the scripted attempt below)

```sh
tests/lab/golden-setup.sh gui          # opens the VM window; auto-login, password admin
```
In the VM, System Settings > Privacy & Security:

1. **Accessibility**: add and enable `Terminal` (and `/usr/libexec/sshd-keygen-wrapper`, press +, then Cmd+Shift+G to type the path).
2. **Automation**: in Terminal run `osascript -e 'tell application "System Events" to keystroke ""'` and click OK on
   "Terminal wants to control System Events".
3. **Screen & System Audio Recording**: enable `Terminal` (screenshots would otherwise show only the wallpaper).
4. Shut the VM down from the Apple menu, then `tests/lab/golden-setup.sh verify` prints a table of what works.

`golden-setup.sh create` prints the same list when it finishes.

### Scripted route: what was tried

**Not yet measured on a real guest.** The probe is `tests/lab/golden-setup.sh tcc-probe` (runs `guest/tcc-probe.sh` in a throw-away clone). It reports
SIP state, whether the user and system `TCC.db` are readable, whether `sudo sqlite3` can insert an Accessibility row for `com.apple.Terminal`, MDM enrolment, and `tccutil` (which can only reset).
Expectation from Apple's design (unverified here): on a stock Cirrus Labs image SIP is *enabled*, so the system `TCC.db` is not writable even as root and the user database is
protected by TCC itself (Full Disk Access needed to write it); writing it would also need `tccd` to reload it. The supported scripted route is an MDM
PPPC profile, which needs an MDM-enrolled guest (out of scope). Until the probe has been run, treat the manual steps above as the only working route.
Disabling SIP in the golden image (Recovery boot, `csrutil disable`, then writing `TCC.db`) is possible in principle with `tart run --recovery`, but is not attempted.

## Building / rebuilding the golden image

```sh
~/.local/bin/tart pull ghcr.io/cirruslabs/macos-sequoia-base:latest   # once; very large download (user `admin`, password `admin`, sshd on)
tests/lab/golden-setup.sh create            # add --force to rebuild; ~15-30 min, mostly downloading CLT + rust
# then the manual steps above, then
tests/lab/golden-setup.sh verify
```
`create` does: clone base to `ashdash-golden` (4 CPUs, 8 GB); boot headless; install the lab ssh key (`~/.local/state/ashdash-lab/id_ed25519`) using
`ssh` with an `SSH_ASKPASS` helper (OpenSSH `SSH_ASKPASS_REQUIRE=force`; no `sshpass`, no `expect`); then `guest/provision.sh` over ssh:
Xcode Command Line Tools if missing, rustup with the host's rustc version (`LAB_RUST_TOOLCHAIN` to override; there is no `rust-toolchain` file) + both darwin targets +
clippy/rustfmt, `pmset` no sleep, no screensaver / lock screen, auto-login check, login keychain unlocked; copies `tests/lab/guest` to `~/lab-guest`; shuts down.
Rebuild after a macOS point release of the base image, a Rust toolchain bump (`rustc --version` on the host), or when the golden image's `~/.cargo/registry` is too stale.

Security notes: the VM is local, NAT-only, reachable only from this host; the admin password is the well-known `admin`. Host key checking is
disabled (`StrictHostKeyChecking=no`, `UserKnownHostsFile=/dev/null`) because every clone gets a new host key and IP; acceptable for local, short-lived VMs,
**not** for anything on a real network. The lab key has no passphrase and only works against the golden image's clones.

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `image 'ashdash-golden' does not exist` | run `tests/lab/golden-setup.sh create` (needs the base image pulled first). |
| `no lab ssh key` / `ssh ... never became ready` | the key is installed during `create`; for a golden image made some other way, copy `~/.local/state/ashdash-lab/id_ed25519.pub` into the guest's `~/.ssh/authorized_keys`. Look at `/tmp/ashdash-lab/logs/<vm>.log`. |
| Job sits at "waiting for a VM slot" | both slots are busy: `python3 tests/lab/slot.py status`. Wait, or find the owner. A slot of a crashed job frees itself; a VM left behind is deleted after 30 min or by `tests/lab/lab-gc --max-age 0` (this deletes *all* run VMs, other agents' too: only when nothing runs). |
| `VZErrorDomain ... The number of VMs exceeds the system limit` | a VM runs outside the lab (a stray `tart run`, UTM, Parallels). `tart list`; `tart stop <name>`. |
| `tart list` shows `ashdash-run-*` leftovers | `tests/lab/lab-down <name>`; only SIGKILL of `lab-with-vm` can leave one, and the watchdog reaps it. |
| `System Events got an error: ... not allowed assistive access` (1002) or `-1743` | TCC grants missing in the golden image: manual steps above. |
| Screenshots only show the wallpaper | Screen Recording not granted to Terminal. |
| `gui-exec` times out | nobody is logged in on the console (auto-login failed): `golden-setup.sh gui`, log in once, re-check `check-golden.sh`. |
| rsync / ssh hangs in `lab-run` | VM crashed or IP changed: `lab-down <vm>` and start over. |
| Disk is filling up | `tart list` for stray VMs; `tart prune --older-than 7` removes cached OCI images (never the golden image, but it would delete the pulled base). |

## Verifying the lab itself

```sh
tests/lab/test/run-tests.sh      # fake tart + fake ssh: slots, queueing, stale recovery, cleanup traps, sync filters, watchdog, naming guards
shellcheck -x -P SCRIPTDIR tests/lab/lab-* tests/lab/*.sh tests/lab/guest/* tests/lab/test/run-tests.sh tests/lab/test/fake-tart   # clean with shellcheck 0.11.0 (pip install shellcheck-py)
```

## Measurements

Not yet measured against a real VM (the ~25 GB base image was still downloading when this was written); `tests/lab/accept.sh` produces the numbers
(clone+boot+ssh+run+delete time for `uname -a`, queueing of a third job, leftover VMs, golden/clone disk use). Paste its report here after the first real run.
Everything above about timing and disk is therefore a description of what the scripts do, not a measurement. Fake-tart tests (`test/run-tests.sh`, ~100 s) pass.

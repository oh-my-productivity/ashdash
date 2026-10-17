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
| `lab-up [--id ID] [--image NAME] [--max-age S]` | gc, wait for a slot, `tart clone ashdash-golden ashdash-run-<id>`, `tart run --no-graphics`, wait for an IP and for ssh. Idempotent for a given `--id`. Prints `<vm> <ip>`. `--max-age S` lets this VM live longer than the default 30 minutes (a soak run): the limit is written to `/tmp/ashdash-lab/max-age/<vm>` so that the watchdog run by *other* `lab-up` calls respects it; `lab-down` removes it. |
| `lab-run <vm> [--artifacts DIR] [--no-sync] [--src DIR] [--env K=V] -- cmd...` | rsync the worktree in (honours `.gitignore`; skips `target/`, `.worktrees/`, `.git`; the guest's `target/` persists between runs), run `cmd` over ssh, stream output, propagate the exit code, copy artifacts out. |
| `lab-down <vm>...` | stop + delete + release the slot. Idempotent; fails loudly if the VM still exists. |
| `lab-with-vm [opts] -- cmd...` | up, run, down with a `trap` for EXIT/INT/TERM/HUP. Exit code is the command's. |
| `lab-smoke [--quick] [--no-gui]` | `lab-with-vm` running `smoke.sh`, artifacts in `lab-out/<timestamp>/`. |
| `lab-gc [--max-age S]` | watchdog: delete `ashdash-run-*` VMs older than 30 min (`LAB_MAX_VM_AGE`). `lab-up` runs it first. |
| `slot.py` | the two-slot semaphore (see below). `slot.py status` shows who holds what. |
| `golden-setup.sh create|gui|verify|manual-steps|tcc-probe` | build and maintain the golden image. |
| `smoke.sh` | the job that runs inside the VM. Grants TCC in the clone first (`LAB_AUTO_TCC=0` to skip, see "TCC grants"). |
| `qa/` | the release-candidate QA scripts (Wave 6): `pkg-install.sh` (DMG, quarantine, Gatekeeper, first launch), `checklist.sh` (docs/QA.md section 3 and the Wave 4/5 behaviours), `soak.sh`, helpers `lib.sh`, `qa-tool.swift`, `ocr.swift`, `fixtures.py`. See docs/QA.md section 2.1 and `docs/qa-runs/`. |
| `guest/` | helpers copied into the golden image (`~/lab-guest`) and used from the synced tree: `gui-exec`, `provision.sh`, `check-golden.sh`, `tcc-probe.sh`, `tcc-grant.sh`. |
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

(Measured on this host, Apple M3 Pro, 12 cores, 36 GB RAM, macOS 26.5.2, guest macOS 15.7.7; see "Measurements" at the end.)

- Disk: the pulled base image and the golden image are full copies (tens of GB each); every `ashdash-run-*` clone is APFS copy-on-write and
  only grows by what the job writes, mainly the guest `target/` directory (a few GB after a first debug build). Clones are deleted by `lab-down`.
- Time: clone is instant; boot to ssh takes about 10 s (measured below); the first `cargo build` in a fresh clone compiles every
  dependency, later `lab-run` calls in the *same* VM are incremental. A one-shot `lab-smoke` therefore pays a full build each time
  (use `--quick`, or rebuild the golden image with a warm `~/.cargo/registry`, see below).
- RAM: 8 GB and 4 CPUs per VM by default (`LAB_VM_MEMORY_MB`, `LAB_VM_CPU` at golden-build time), 16 GB for the two slots.

## TCC grants (Accessibility, Automation, Screen Recording)

macOS asks for these interactively and attributes them to the *responsible* app. Two routes reach the logged-in GUI session:
`tests/lab/guest/gui-exec -- <cmd>` opens a `.command` file with **Terminal.app** (responsible app: Terminal), and a plain ssh session
(responsible app on macOS 15: `com.apple.sshd-session`). `smoke.sh` uses gui-exec to start the app and the ssh session to press keys and take
screenshots (gui-exec brings no window to the front any more, but a launcher panel hides when it loses focus, so keys are sent from ssh).

### Scripted grant (default, verified on the real guest)

The cirruslabs base image ships with **SIP disabled** (`csrutil status` in the guest), so `TCC.db` is writable by root. `tests/lab/guest/tcc-grant.sh`
writes Accessibility, Post-Event, Screen Recording (system DB) and Automation to System Events / Finder (user DB) for `com.apple.Terminal`,
`com.apple.sshd-session`, `/usr/libexec/sshd-keygen-wrapper` and `/usr/bin/osascript`, seeds the macOS 15 "bypass the system private window picker"
approvals (`~/Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist`, next-ask date ten years out; the stock prompt returns every
30 days) and restarts `tccd`/`replayd`. `smoke.sh` runs it **inside the throw-away clone** before its first System Events call; the golden image is not
modified. `LAB_AUTO_TCC=0 tests/lab/lab-smoke ...` turns it off (then the hotkey stage SKIPs with the reason). Order matters: after a System Events call has
already raised the consent dialog, granting afterwards did not recover (tccd/Terminal kept waiting), so grant first.
On a guest with SIP on the script stops with an error and the manual route below is the only one.

### Manual steps (optional: only if the golden image itself should carry the grants)

```sh
tests/lab/golden-setup.sh gui          # opens the VM window; auto-login, password admin
```
In the VM, System Settings > Privacy & Security:

1. **Accessibility**: press +, add `Terminal`, switch it on (`sshd-keygen-wrapper` and `osascript` are already granted in the base image).
2. **Automation**: in Terminal run `osascript -e 'tell application "System Events" to keystroke ""'` and click OK on "Terminal wants to control System Events".
3. **Screen & System Audio Recording**: switch `Terminal` on; then run `screencapture -x /tmp/t.png` in Terminal and click **Allow** on the macOS 15 prompt
   "Terminal is requesting to bypass the system private window picker..." (without it screenshots show only the wallpaper and the call blocks the first time).
4. The same prompt appears for commands run over ssh (`com.apple.sshd-session`): in Terminal run `ssh admin@localhost screencapture -x /tmp/s.png` (password `admin`) and click Allow.
5. Shut the VM down from the Apple menu, then `tests/lab/golden-setup.sh verify` prints a table of what works (`=> TCC` row; the `screencapture` row only proves a file was written).

macOS asks the prompt of step 3 again after 30 days, so a hand-made grant expires; that is the reason the scripted route is the default.
`golden-setup.sh create` prints the same list when it finishes.

### What was tried (first real run, 2026-10-17)

`golden-setup.sh tcc-probe` in a clone: SIP **disabled**; both `TCC.db` files readable and writable with `sudo sqlite3` (inserts succeeded); no MDM
enrolment; `tccutil` only has `reset`. The base image already contains Accessibility/Screen/Post-Event/Automation rows for `sshd-keygen-wrapper`,
`osascript`, `tart-guest-agent` and `python` (so System Events works from ssh out of the box), but none for `com.apple.sshd-session` or Terminal. Without
any grant: a first `screencapture` over ssh pops "com.apple.sshd-session is requesting to bypass the system private window picker" on the VM screen
(shot is the wallpaper plus that dialog), and `osascript` System Events from Terminal blocks on the Automation dialog (gui-exec times out, rc 124).
With `tcc-grant.sh` on a fresh clone: System Events from Terminal returns `Terminal`, screenshots from both Terminal and ssh show the windows, no dialog.

## Building / rebuilding the golden image

```sh
~/.local/bin/tart pull ghcr.io/cirruslabs/macos-sequoia-base:latest   # once; very large download (user `admin`, password `admin`, sshd on)
tests/lab/golden-setup.sh create            # add --force to rebuild; about 1 min on the cirruslabs base (CLT already inside; rust download dominates)
# manual steps above are optional (smoke.sh grants in the clone); then
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
| `System Events got an error: ... not allowed assistive access` (1002), `-1743`, or `gui-exec: timed out` on an osascript call | TCC grant missing (the call waits on an invisible consent dialog): run `smoke.sh` (auto-grant) or `guest/tcc-grant.sh` first; do not retry in the same VM after a dialog appeared, use a fresh clone. |
| `No application knows how to open URL ashdash://...` (-10814) | the app bundle lives under `$TMPDIR` (`/var/folders/...`); LaunchServices ignores it. Build it under the guest home (smoke.sh does). |
| Launcher window vanishes between the hotkey and the check | something activated another app (every `gui-exec` call used to bring Terminal to the front); drive keys/screenshots from the ssh session. |
| A "requesting to bypass the system private window picker" dialog in screenshots | macOS 15 screen-capture approval for Terminal / `com.apple.sshd-session`; `tcc-grant.sh` pre-seeds it. |
| Screenshots only show the wallpaper | Screen Recording not granted to the responsible app (Terminal / `com.apple.sshd-session`): `tcc-grant.sh`. |
| Many Terminal windows pile up in screenshots | each `gui-exec` job leaves its window (`[Process completed]`); harmless, the clone is deleted. |
| `gui-exec` times out | nobody is logged in on the console (auto-login failed): `golden-setup.sh gui`, log in once, re-check `check-golden.sh`. |
| rsync / ssh hangs in `lab-run` | VM crashed or IP changed: `lab-down <vm>` and start over. |
| Disk is filling up | `tart list` for stray VMs; `tart prune --older-than 7` removes cached OCI images (never the golden image, but it would delete the pulled base). |

### Things the Wave 6 QA runs learned about the guest

* **Gatekeeper is off in the base image** (`spctl --status`: assessments disabled). `qa/pkg-install.sh` runs `sudo spctl --master-enable` in the clone before it tests quarantine.
* **Spotlight indexing is off** (`mdutil -s /`), so `mdfind`-backed features (`open`, `find`, Open With...) return nothing. `sudo mdutil -i on /` starts indexing but it had not produced results 40 minutes later.
* **SIP is off, and TCC does not protect `~/Library/Safari`** in this guest: a process without Full Disk Access reads `Bookmarks.plist`, so the Full Disk Access denial cannot be reproduced here.
* **`pmset displaysleepnow` locks the session** and nothing in the lab can unlock it (System Events keystrokes do not reach the login window; `sysadminctl -screenLock off` did not help). Recover with `sudo shutdown -r now` (auto-login brings the session back). Do not call it before the end of a run.
* The display is 1024x768 at 2x backing scale. Windows are listed with `scripts/qa/windows.swift` (owner `Ashdash` for the bundle, `ashdash` for a bare binary); a freshly started app also owns a hidden, launcher-sized window at layer 3: only layer 25 means "shown".
* A modifier chord posted in one go (`key code N using {option down}`) reaches the app within one 16 ms frame with the modifier already released: hold the modifier with `key down` for ~0.3 s (`chord` in `qa/lib.sh`). `keystroke` of non-ASCII text is mangled by System Events: use `qa-tool type`.
* To make the app its own TCC client (prompts name "Ashdash"), start it with `open` (`gui-exec -- open --env HOME=... Ashdash.app`); started as a child of Terminal the prompts name Terminal.

## Verifying the lab itself

```sh
tests/lab/test/run-tests.sh      # fake tart + fake ssh: slots, queueing, stale recovery, cleanup traps, sync filters, watchdog, naming guards
shellcheck -x -P SCRIPTDIR tests/lab/lab-* tests/lab/*.sh tests/lab/guest/* tests/lab/test/run-tests.sh tests/lab/test/fake-tart   # clean with shellcheck 0.11.0 (pip install shellcheck-py)
```

## Measurements

Measured 2026-10-17 on the host above (Apple M3 Pro, 12 cores, 36 GB, macOS 26.5.2; Tart 2.41.0), guest macOS 15.7.7, 4 CPUs / 8 GB per VM, rustc 1.95.0.
The host was **not idle**: other agents were building and testing the whole time (load average 3-6 without any VM), so the variance below is real.

| What | Result |
|---|---|
| Base image `macos-sequoia-base:latest` | pulled earlier; `tart list`: disk 50 GB, size 33 GB; 31 GB on disk in `~/.tart/cache/OCIs` |
| `golden-setup.sh create` (clone, boot, ssh key, provision, helpers, shutdown) | **54 s** (CLT were already in the image; the rustup + 1.95.0 download was the long part) |
| Golden image disk | `tart list`: disk 50 GB, size 31 GB; `du`: 29 GB. Host `df` after golden + base: 477 GiB free of 926 GiB |
| `lab-up` (APFS clone, headless boot, IP, ssh ready) | **10 s** (10.2 s measured) |
| `lab-with-vm -- uname -a` end to end (clone + boot + ssh + run + delete) | **13.5 s** |
| `golden-setup.sh verify` / `tcc-probe` end to end | 49 s / 13.5 s (verify includes one 30 s gui-exec timeout before the TCC fix) |
| Sync of the worktree (3.1 MB; first and second sync) | **0.6-0.7 s** including the ssh round trip |
| Cold `cargo build -p ashdash` (debug, fresh clone, deps from crates.io) | **33 s** on a quiet moment, 46 s, 78 s and 114 s while other agents compiled on the host |
| Warm no-op `cargo build` / after touching `crates/core/src/lib.rs` | 0.4 s / 1.3 s |
| `scripts/check.sh` in the guest (fmt, clippy, all tests, after the cold build) | **92 s** (with `/tmp/x` present, see "Known failure" below) |
| `build-app.sh` native bundle (cold / warm) | 76-78 s / 1 s |
| `lab-smoke --quick` (everything incl. GUI stages) | 4 min 11 s - 4 min 32 s; two of them concurrently each took about the same |
| `lab-smoke` (with `check.sh`) | 4 min 09 s |
| `tests/lab/accept.sh` (one `uname`, three concurrent jobs on two slots, cleanup) | 91 s: the third job started 29 s after the second, never more than 2 VMs, no `ashdash-run-*` left, slots free |
| Disk per run VM | clone is copy-on-write: `tart list` size 34 GB vs 31 GB golden after a full debug build + bundle (+3 GB; guest `target/` is 4.0 GB); `du` on a clone shows the logical 31 GB, ignore it. Gone after `lab-down` |
| Guest root disk | 46 GiB total, 15 GiB free after the above |
| Host load with two VMs compiling | load average 5 (baseline, other agents) rising to 9-10 on 12 cores while both guests built; `tart run` processes themselves show ~0 % CPU in `ps` (the work is accounted to the VM) |
| RAM | 2 x 8 GB for two VMs; host stayed responsive |

### Real-VM results of `smoke.sh` (this branch)

With the scripted TCC grant: env, build, cli-query, bundle, gatekeeper (informational), gui-launch, cli-show, cli-search, url-scheme, tcc-grant,
system-events, **hotkey** (Option+Shift+Space toggles the launcher: 0 -> 1 windows, window 700 x 264 at 162,154 with 4 results for "saf"),
screenshot, login-item, quit: all PASS. Without it (`LAB_AUTO_TCC=0`): same, but hotkey and window-state are SKIP with
"System Events denied ... gui-exec: timed out after 20s" and the job exits 0.

### Known failure found by the lab (not a lab bug)

`scripts/check.sh` fails in a clean guest: `engine::tests::universal_actions_are_never_written_to_usage` (`crates/core/src/engine.rs:1241`) asserts that
`Action::Open("/tmp/x")` is learned, which only happens if `/tmp/x` exists. It passes on any host that happens to have that file and passes in the guest after
`touch /tmp/x`. The test should create its own temp file.

#!/usr/bin/env python3
"""Slot semaphore for the Ashdash test lab.

Apple allows at most two concurrent macOS guests on an Apple Silicon host, so VM jobs share
two slots. A slot is a file under <run-dir>/slots/ locked with fcntl.flock (the kernel drops
the lock when the holder dies, so stale locks recover by themselves; macOS ships no flock(1)).

    slot.py acquire --name N [--timeout S] [--max-age S] [--vm-name V] [--no-delete]
        Queue until a slot is free, then leave a detached holder process that owns the lock.
        Prints the slot index on stdout. The holder exits (releasing the slot) when:
          * `slot.py release --name N` sends it SIGTERM,
          * the watched VM was seen and then disappeared,
          * the VM never appeared within --grace seconds (lab-up died),
          * --max-age elapsed (the holder also stops and deletes the VM, unless --no-delete).
    slot.py release --name N     Idempotent.
    slot.py holds --name N       Exit 0 if a live holder owns a slot under that name.
    slot.py status               One line per slot (free / held by NAME since ...).
"""
import argparse
import errno
import fcntl
import json
import os
import signal
import subprocess
import sys
import time


def slot_path(run_dir, i):
    return os.path.join(run_dir, "slots", "slot%d.lock" % i)


def read_record(fd):
    try:
        os.lseek(fd, 0, os.SEEK_SET)
        data = os.read(fd, 4096).decode("utf-8", "replace").strip()
        return json.loads(data) if data else {}
    except (OSError, ValueError):
        return {}


def write_record(fd, rec):
    os.ftruncate(fd, 0)
    os.lseek(fd, 0, os.SEEK_SET)
    os.write(fd, json.dumps(rec).encode())


def try_lock(path):
    fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return fd
    except OSError as e:
        os.close(fd)
        if e.errno in (errno.EWOULDBLOCK, errno.EAGAIN, errno.EACCES):
            return None
        raise


def is_holder(pid):
    """True if pid is alive and is one of our holder processes (guards against pid reuse)."""
    try:
        out = subprocess.run(["ps", "-o", "command=", "-p", str(pid)], capture_output=True, text=True).stdout
    except OSError:
        return False
    return "slot.py" in out and "--holder" in out


def tart_vm_exists(tart, name):
    try:
        out = subprocess.run([tart, "list", "--source", "local", "--quiet"], capture_output=True, text=True, timeout=30).stdout
    except (OSError, subprocess.TimeoutExpired):
        return None  # unknown: do not act on it
    return name in out.split()


def holder_loop(fd, args):
    stop = []
    signal.signal(signal.SIGTERM, lambda *_: stop.append("term"))
    signal.signal(signal.SIGINT, lambda *_: stop.append("int"))
    signal.signal(signal.SIGHUP, signal.SIG_IGN)
    start = time.time()
    seen = False
    reason = None
    while not stop:
        time.sleep(1)
        now = time.time()
        if now - start > args.max_age:
            reason = "max-age"
            break
        if args.vm_name and int(now) % 5 == 0:
            exists = tart_vm_exists(args.tart, args.vm_name)
            if exists:
                seen = True
            elif exists is False:
                if seen:
                    reason = "vm-gone"
                    break
                if now - start > args.grace:
                    reason = "vm-never-appeared"
                    break
    if reason == "max-age" and args.vm_name and not args.no_delete:
        subprocess.run([args.tart, "stop", args.vm_name, "--timeout", "5"], capture_output=True)
        subprocess.run([args.tart, "delete", args.vm_name], capture_output=True)
    try:
        write_record(fd, {})
    finally:
        os.close(fd)


def cmd_acquire(args):
    deadline = time.time() + args.timeout
    last_note = 0
    while True:
        for i in range(args.slots):
            fd = try_lock(slot_path(args.run_dir, i))
            if fd is None:
                continue
            write_record(fd, {"name": args.name, "slot": i, "since": int(time.time()), "pid": None})
            r, w = os.pipe()
            pid = os.fork()
            if pid:  # parent: wait until the holder has re-recorded its own pid, print slot, exit
                os.close(w)
                os.close(fd)
                os.read(r, 1)
                print(i)
                return 0
            # child: become the detached holder
            os.close(r)
            os.setsid()
            if os.fork():
                os._exit(0)
            write_record(fd, {"name": args.name, "slot": i, "since": int(time.time()), "pid": os.getpid()})
            os.write(w, b"x")
            os.close(w)
            devnull = os.open(os.devnull, os.O_RDWR)
            for std in (0, 1, 2):
                os.dup2(devnull, std)
            try:
                os.set_inheritable(fd, True)
                os.execv(sys.executable, [sys.executable, os.path.abspath(__file__), "--holder", str(fd)] + sys.argv[1:])
            except OSError:
                holder_loop(fd, args)
                os._exit(0)
        if time.time() > deadline:
            print("slot: no free VM slot after %ds" % args.timeout, file=sys.stderr)
            return 3
        if time.time() - last_note > 30:
            last_note = time.time()
            print("slot: waiting for a free VM slot (%s)" % describe(args), file=sys.stderr)
        time.sleep(1)


def describe(args):
    parts = []
    for i in range(args.slots):
        fd = try_lock(slot_path(args.run_dir, i))
        if fd is None:
            fd2 = os.open(slot_path(args.run_dir, i), os.O_RDONLY)
            rec = read_record(fd2)
            os.close(fd2)
            parts.append("slot%d: %s %ds" % (i, rec.get("name", "?"), int(time.time() - rec.get("since", time.time()))))
        else:
            os.close(fd)
            parts.append("slot%d: free" % i)
    return ", ".join(parts)


def cmd_release(args):
    released = False
    for i in range(args.slots):
        path = slot_path(args.run_dir, i)
        if not os.path.exists(path):
            continue
        fd = os.open(path, os.O_RDONLY)
        rec = read_record(fd)
        os.close(fd)
        if rec.get("name") == args.name and rec.get("pid") and is_holder(rec["pid"]):
            try:
                os.kill(rec["pid"], signal.SIGTERM)
                released = True
            except OSError:
                pass
    # Wait briefly so a following acquire sees the slot free.
    for _ in range(50):
        if not any_held_by(args):
            break
        time.sleep(0.1)
    return 0 if released or not any_held_by(args) else 1


def any_held_by(args):
    for i in range(args.slots):
        path = slot_path(args.run_dir, i)
        if not os.path.exists(path):
            continue
        fd = try_lock(path)
        if fd is not None:  # free
            os.close(fd)
            continue
        fd2 = os.open(path, os.O_RDONLY)
        rec = read_record(fd2)
        os.close(fd2)
        if rec.get("name") == args.name:
            return True
    return False


def cmd_holds(args):
    return 0 if any_held_by(args) else 1


def cmd_status(args):
    print(describe(args))
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--holder", type=int, help=argparse.SUPPRESS)
    ap.add_argument("--run-dir", default="/tmp/ashdash-lab")
    ap.add_argument("--slots", type=int, default=2)
    sub = ap.add_subparsers(dest="cmd", required=True)
    a = sub.add_parser("acquire")
    a.add_argument("--name", required=True)
    a.add_argument("--timeout", type=int, default=2700)
    a.add_argument("--max-age", type=int, default=1800)
    a.add_argument("--vm-name", default="")
    a.add_argument("--grace", type=int, default=600)
    a.add_argument("--no-delete", action="store_true")
    a.add_argument("--tart", default="tart")
    r = sub.add_parser("release")
    r.add_argument("--name", required=True)
    sub.add_parser("status")
    h = sub.add_parser("holds")
    h.add_argument("--name", required=True)
    args = ap.parse_args()
    os.makedirs(os.path.join(args.run_dir, "slots"), mode=0o700, exist_ok=True)
    if args.holder is not None:
        holder_loop(args.holder, args)
        return 0
    return {"acquire": cmd_acquire, "release": cmd_release, "status": cmd_status, "holds": cmd_holds}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())

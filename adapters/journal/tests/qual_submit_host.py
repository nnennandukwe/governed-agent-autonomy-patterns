"""Provision the shipping journal and exercise its public commands in fresh processes."""

import contextlib
import ctypes
import fcntl
import json
import os
from pathlib import Path
import selectors
import signal
import struct
import subprocess
import sys
import time

from qual_executor_quiescence import record_file
from qual_host import Slot, canonical, digest, identity, write_private


def subject(workspace):
    """Hash the three original files using the shipping reference's snapshot representation."""
    files = {}
    for name in ("TASK.md", "shipping.py", "deployment.json"):
        with (workspace / name).open("rb") as stream:
            info = os.fstat(stream.fileno())
            files[name] = {"content": stream.read().decode(), "identity": {
                "device": info.st_dev, "inode": info.st_ino, "mode": info.st_mode & 0o777}}
    return digest(canonical(files))


def sync_dir(path):
    fd = os.open(path, os.O_DIRECTORY | os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def update_grant(directory, **changes):
    """Acknowledge a grant update only after exclusive lock, atomic replacement and sync."""
    with (directory / "authority.lock").open("rb") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        path = directory / "authority.json"
        previous = json.loads(path.read_bytes())
        current = {**previous, **changes, "revision": previous["revision"] + 1}
        stage = directory / "next.json"
        with stage.open("xb") as stream:
            os.fchmod(stream.fileno(), 0o600)
            stream.write(canonical(current))
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(stage, path)
        sync_dir(directory)


class Shipping:
    def __init__(self, root, health, args, inputs, profile, budget, content, key):
        self.root, self.key, self.args = root, key, args
        self.commands = []
        self.workspace = root / "workspace"
        self.workspace.mkdir(mode=0o700)
        for name, data in inputs.items():
            write_private(self.workspace / name, data)
        self.slot = Slot(root, health, args.release, args.qualification, self.workspace, profile)
        self.target = self.workspace / "shipping.py"
        self.initial = record_file(self.target)
        self.proposal = {"path": "shipping.py", "content": content, "plan": "Implement shipping rules.",
                         "base_digest": subject(self.workspace)}
        self.authority = health / "authority"
        self.archive = root / "archive"
        for directory in (self.authority, self.archive):
            directory.mkdir(mode=0o700)
        self.authority_lock = self.authority / "authority.lock"
        write_private(self.authority_lock, b"")
        self.slot.host["live"] = {"can_submit": True, "authority": identity(self.authority),
                                  "authority_lock": identity(self.authority_lock), "archive": identity(self.archive)}
        self.slot.configure()
        now = time.time_ns() // 1_000_000
        pins = self.slot.host["slot"]
        source = b"shipping workspace owner approved this file replacement"
        self.grant = {
            "format": "gaap.shipping-authority/0.1", "revision": 1,
            **{k: pins[k] for k in ("workspace_id", "store_id", "profile_digest")},
            "principal": "alice", "key": key, "proposal_digest": digest(canonical(self.proposal)),
            "target": "shipping.py", "operation": "filesystem.write", "can_execute": True,
            "revoked": False, "cancelled": False, "allow_retention": True, "valid_from_ms": now - 2000,
            "expires_at_ms": now + 120_000, "budget": budget,
            "approval": {"approval_id": "approval-shipping", "actor_id": profile["grant_observer"],
                         "scope": "filesystem.write:shipping.py", "subject_digest": self.proposal["base_digest"],
                         "evidence": {"evidence_type": "approval", "digest": digest(source),
                                      "locator": "gaap:issuer/approval-shipping"}},
            "approval_bytes_hex": source.hex()}
        write_private(self.authority / "authority.json", canonical(self.grant))
        enrolled = self.slot.call("enroll", expected="enrolled")
        self.commands.append({"command": "enroll", "reply": enrolled})

    def lookup(self, key, *, expected):
        reply = self.slot.call("lookup", key, expected=expected)
        self.commands.append({"command": "lookup", "principal": self.slot.host["principal"],
                              "key": key, "reply": reply})
        return reply

    def start(self, *, key=None, point=None, action="stop", trace=(), binary=None, fail_cleanup=False, supervisor_fds=()):
        """Launch the real submission command through the existing descriptor handoff."""
        binary = binary or (self.args.fault_reference if point else self.args.reference)
        handoff = "operator:" + ":".join(f"{target}={source}" for target, source in supervisor_fds) if supervisor_fds else "normal"
        argv = [sys.executable, "-B", str(Path(__file__).with_name("qual_host.py")),
                str(self.slot.config), str(self.slot.health), handoff,
                *map(str, trace), str(binary), "journal-submit", key or self.key]
        env = dict(os.environ)
        if fail_cleanup:
            env["GAAP_JOURNAL_CLEANUP_FAIL"] = "1"
        if point:
            env.update(GAAP_JOURNAL_POINT=point, GAAP_JOURNAL_ACTION=action)
        child = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 env=env, start_new_session=True, pass_fds=tuple(source for _, source in supervisor_fds))
        self.commands.append({"command": "journal-submit", "principal": self.slot.host["principal"],
                              "key": key or self.key, "binary": str(binary), "pid": child.pid, "cut": point})
        return child

    def finish(self, child, stdin=None):
        """Require JSON and preserve diagnostics if the command dies without a result."""
        try:
            out, err = child.communicate(stdin, timeout=20)
        except BaseException:
            terminate(child)
            raise
        try:
            result = json.loads(out or err)
        except ValueError as exc:
            raise AssertionError((child.returncode, out[:300], err[:1500])) from exc
        self.commands.append({"pid": child.pid, "exit": child.returncode, "reply": result})
        return child.returncode, result

    def submit(self, *, stdin=None, **options):
        return self.finish(self.start(**options), canonical(self.proposal) if stdin is None else stdin)

    @contextlib.contextmanager
    def stopped(self, point, **options):
        """Check the named stop and wait for the kernel to suspend this exact worker."""
        child = self.start(point=point, **options)
        try:
            child.stdin.write(canonical(self.proposal))
            child.stdin.close()
            child.stdin = None
            with selectors.DefaultSelector() as poller:
                poller.register(child.stderr, selectors.EVENT_READ)
                if not poller.select(12):
                    raise AssertionError(("worker missed stop", point))
                line = child.stderr.readline().decode().strip()
            if line != "journal qualification stop " + point:
                raise AssertionError(("unexpected stop", point, line, child.poll()))
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                pid, status = os.waitpid(child.pid, os.WUNTRACED | os.WNOHANG)
                if pid and os.WIFSTOPPED(status) and os.WSTOPSIG(status) == signal.SIGSTOP:
                    break
                time.sleep(.005)
            else:
                raise AssertionError(("no kernel stop", point))
            yield child
        finally:
            terminate(child)
            self.commands.append({"cut": point, "pid": child.pid, "exit": child.returncode})


def terminate(child):
    if child.poll() is None:
        child.kill()
        try:
            child.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate(timeout=5)


def lock_blocked(path):
    """Check a separately opened lock description without entering the writer's critical section."""
    with path.open("rb") as stream:
        try:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return True
        return False


@contextlib.contextmanager
def watch_publications(f):
    """Order target and journal replacement notifications from both directories on one kernel queue."""
    libc = ctypes.CDLL(None, use_errno=True)
    fd = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
    if fd < 0:
        raise OSError(ctypes.get_errno(), "inotify_init1")
    try:
        watches = {}
        for path in (f.target, f.slot.journal):
            wd = libc.inotify_add_watch(fd, os.fsencode(path.parent), 0x80)  # IN_MOVED_TO
            if wd < 0:
                raise OSError(ctypes.get_errno(), "inotify_add_watch")
            watches[wd] = path.name

        def drain():
            ordered = []
            while True:
                try:
                    data = os.read(fd, 8192)
                except BlockingIOError:
                    return ordered
                offset = 0
                while offset < len(data):
                    watch, mask, cookie, size = struct.unpack_from("iIII", data, offset)
                    name = os.fsdecode(data[offset + 16:offset + 16 + size].split(b"\0", 1)[0])
                    if watch not in watches or mask & (0x4000 | 0x8000):
                        raise AssertionError(("lost qualification watch", watch, mask))
                    if mask & 0x80 and name == watches[watch]:
                        if not cookie:
                            raise AssertionError(("missing rename cookie", name))
                        ordered.append(name)
                    offset += 16 + size
        yield drain
    finally:
        os.close(fd)

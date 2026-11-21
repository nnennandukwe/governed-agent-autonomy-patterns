"""Provision a single disposable slot and observe the production CLI out of process."""

import hashlib
import json
import os
from pathlib import Path
import selectors
import signal
import shutil
import stat
import subprocess
import sys
import time
import uuid


LIMITS = {
    "anchor_bytes": 16 * 1024,
    "journal_bytes": 8 * 1024 * 1024,
    "attempts": 12,
    "raw_proposal_bytes": 256 * 1024,
    "canonical_proposal_bytes": 256 * 1024,
    "depth": 64,
}


def canonical(value):
    """Encode fixture values with ASCII field names, strings and safe integers."""
    return json.dumps(value, sort_keys=True, ensure_ascii=False, allow_nan=False,
                      separators=(",", ":")).encode()


def digest(data):
    """Hash independently observed bytes with the frozen digest prefix."""
    return "sha256:" + hashlib.sha256(data).hexdigest()


def identity(path):
    """Observe a file through lstat, independently of the adapter."""
    info = path.lstat()
    return {"path": str(path), "dev": str(info.st_dev), "ino": str(info.st_ino),
            "uid": info.st_uid, "mode": stat.S_IMODE(info.st_mode)}


def snapshot(path):
    """Record canonical entry identities and bytes, including an absent entry."""
    return (identity(path), digest(path.read_bytes())) if path.exists() else None


def observe_tree(path):
    """Capture directories and files so a forbidden mutation is observable."""
    observed = {}
    for entry in sorted(path.rglob("*")):
        observed[str(entry.relative_to(path))] = (
            identity(entry) if entry.is_dir() else snapshot(entry))
    return observed


def write_private(path, data):
    """Publish test input and sync its directory before opening another process."""
    with path.open("wb") as stream:
        os.fchmod(stream.fileno(), 0o600)
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def mount_id(path):
    """Use the kernel's FD mount ID, including for roots inside a mount point."""
    fd = os.open(path, os.O_DIRECTORY | os.O_RDONLY)
    try:
        lines = Path(f"/proc/self/fdinfo/{fd}").read_text().splitlines()
        return int(next(line.split()[1] for line in lines if line.startswith("mnt_id:")))
    finally:
        os.close(fd)


def named(name):
    """Create a typed fixture identity with a reproducible digest."""
    return {"name": name, "version": "1", "digest": digest(name.encode())}


def base_profile():
    """Supply independent configured roles before an empty store is enrolled."""
    return {"format": "gaap.local-file-history/0.1",
            "policies": [named("policy")], "capability": named("capability"),
            "tool_schema_digest": digest(b"tool"), "sandbox": named("sandbox"),
            "executor": named("executor"), "implementer_id": "implementer",
            "verifier_id": "verifier", "grant_observer": "grant-host",
            "effect_observer": "file-host", "require_grant": False}


class Slot:
    """Represent a trusted test provisioner and its independent volume-health file."""

    def __init__(self, root, health_root, release, qualification, workspace=None, profile=None):
        """Create the private enrollment bindings before any journal process starts."""
        self.root = root
        self.workspace = workspace or root / "workspace"
        self.control = root / "control"
        self.store = root / "store"
        for path in [self.control, self.store]:
            path.mkdir(mode=0o700)
        if workspace is None:
            self.workspace.mkdir(mode=0o700)
            write_private(self.workspace / "sentinel", b"owned by the qualification harness\n")
        self.lock = self.control / "lock"
        self.anchor = self.control / "anchor.json"
        self.journal = self.store / "journal.json"
        self.config = root / "host.json"
        self.health = health_root / uuid.uuid4().hex
        self.release, self.qualification = release, qualification
        write_private(self.lock, b"")
        write_private(self.health, b"0")
        if self.health.stat().st_dev == self.store.stat().st_dev:
            raise AssertionError("qualification health latch must use a different device")
        profile = profile or base_profile()
        pins = {"workspace_id": uuid.uuid4().hex, "store_id": uuid.uuid4().hex,
                "profile_digest": digest(canonical(profile)), "workspace": identity(self.workspace),
                "control": identity(self.control), "store": identity(self.store),
                "lock": identity(self.lock)}
        self.unused = {**pins, "format": "gaap.journal-anchor/0.1",
                       "limits": LIMITS.copy(), "phase": "unused", "generation": None,
                       "journal_digest": None}
        self.host = {"slot": {**pins, "health": identity(self.health),
                              "mount_id": mount_id(self.store)},
                     "principal": "alice", "can_enroll": True, "can_read": True,
                     "profile": profile, "archive": {"entries": []}}
        write_private(self.anchor, canonical(self.unused))
        self.configure()

    def configure(self):
        """Give each child its own read-only descriptor to the host handoff."""
        write_private(self.config, canonical(self.host))

    def install_history(self, attempts, generation=1):
        """Stage test histories as a trusted fixture producer, independent of lookup."""
        journal = {name: self.unused[name] for name in ["workspace_id", "store_id", "profile_digest"]}
        journal.update(format="gaap.journal/0.1", attempts=attempts, generation=generation)
        data = canonical(journal)
        write_private(self.journal, data)
        write_private(self.anchor, canonical({**self.unused, "phase": "active",
                                               "generation": generation, "journal_digest": digest(data)}))

    def start(self, args, *, point=None, action=None, trace=None, fail_cleanup=False,
              handoff="normal", fail_sync_ordinal=None, sync_errno="EIO", read_trace=None,
              fail_read_ordinal=None, child_umask=-1, fail_stage_chmod=False):
        """Launch an independent host process with the journal's real descriptor handoff."""
        binary = self.qualification if point else self.release
        command = [str(binary), *args]
        if read_trace:
            # Trace the release CLI after the Python supervisor has opened fd 3 and fd 4.
            tracer = shutil.which("strace")
            if tracer is None:
                raise AssertionError("journal qualification requires strace")
            tracing = [tracer, "-qq", "-yy", "-o", str(read_trace), "-e", "trace=read"]
            if fail_read_ordinal is not None:
                tracing += ["-e", f"inject=read:error=EIO:when={fail_read_ordinal}"]
            command = [*tracing, *command]
        argv = [sys.executable, "-B", str(Path(__file__).resolve()), str(self.config),
                str(self.health), handoff, *command]
        if trace:
            calls = "fsync,renameat,renameat2,rename"
            if args[0] == "lookup":
                calls += ",openat,openat2,unlinkat,linkat,ftruncate,pwrite64"
            if fail_stage_chmod:
                calls = "fchmod"
            tracing = ["strace", "-qq", "-yy", "-o", str(trace), "-e", "trace=" + calls]
            if fail_stage_chmod:
                tracing += ["-e", "inject=fchmod:error=EPERM:when=1"]
            if fail_sync_ordinal is not None:
                # -D keeps the tracee as our child for waitpid, SIGKILL and cleanup.
                tracing += ["-D", "-e", f"inject=fsync:error={sync_errno}:when={fail_sync_ordinal}"]
            argv = [*tracing, *argv]
        env = os.environ.copy()
        for name in ("GAAP_JOURNAL_POINT", "GAAP_JOURNAL_ACTION", "GAAP_JOURNAL_CLEANUP_FAIL"):
            env.pop(name, None)
        if point:
            env.update(GAAP_JOURNAL_POINT=point, GAAP_JOURNAL_ACTION=action)
        if fail_cleanup:
            env["GAAP_JOURNAL_CLEANUP_FAIL"] = "1"
        return subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, env=env, umask=child_umask)

    def call(self, *args, expected, stdin=None, **options):
        """Check both the public JSON status and its matching process exit behavior."""
        child = self.start(args, **options)
        try:
            out, err = child.communicate(stdin, timeout=12)
        except BaseException:
            child.kill()
            child.communicate()
            raise
        try:
            reply = json.loads(out or err)
        except ValueError as exc:
            raise AssertionError((args, child.returncode, out[:1200], err[:1200])) from exc
        allowed = {expected} if isinstance(expected, str) else set(expected)
        if reply.get("status") not in allowed:
            raise AssertionError((args, allowed, child.returncode, reply))
        if reply["status"] in {"enrolled", "absent", "partial", "published", "conflict"}:
            if child.returncode:
                raise AssertionError((args, child.returncode, reply))
        elif child.returncode == 0 or not reply.get("reason"):
            raise AssertionError((args, "rejection requires a failure exit and reason", reply))
        return reply

    def stop(self, point, **options):
        """Wait for a named hook and confirm the writer actually entered SIGSTOP."""
        child = self.start(["enroll"], point=point, action="stop", **options)
        try:
            with selectors.DefaultSelector() as sel:
                sel.register(child.stderr, selectors.EVENT_READ)
                if not sel.select(timeout=6):
                    raise AssertionError(("unreached hook", point))
                line = child.stderr.readline().decode(errors="replace").strip()
            if line != "journal qualification stop " + point:
                raise AssertionError((point, line, child.poll()))
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                pid, status = os.waitpid(child.pid, os.WNOHANG | os.WUNTRACED)
                if pid and os.WIFSTOPPED(status) and os.WSTOPSIG(status) == signal.SIGSTOP:
                    return child
                time.sleep(.005)
            raise AssertionError(("writer did not stop", point))
        except BaseException:
            child.kill()
            child.communicate()
            raise


def exec_with_host_fds(args):
    """Open existing files in the required modes and grant only the two selected FDs."""
    import fcntl
    pins, health, handoff, command, *rest = args
    descriptors = [(pins, os.O_RDWR if handoff == "writable-config" else os.O_RDONLY, 3)]
    if handoff != "missing-health":
        descriptors.append((health, os.O_RDONLY if handoff == "readonly-health" else os.O_RDWR, 4))
    for name, flags, target in descriptors:
        opened = os.open(name, flags | os.O_NOFOLLOW)
        saved = fcntl.fcntl(opened, fcntl.F_DUPFD_CLOEXEC, 10)
        os.close(opened)
        try:
            os.dup2(saved, target, inheritable=True)
        finally:
            os.close(saved)
    os.execv(command, [command, *rest])


if __name__ == "__main__":
    exec_with_host_fds(sys.argv[1:])

"""Observe the one shipping worker from before its launch through its pidfd exit."""

import contextlib
import copy
import ctypes
import fcntl
import json
import os
from pathlib import Path
import select
import signal
import socket
import stat
import struct
import time

from qual_host import canonical, identity, write_private
from qual_submit_host import sync_dir, terminate


def tracked(workspace):
    """Read the three shipping inputs and their file identities for the observer's repository snapshot."""
    files = {}
    for name in ("TASK.md", "shipping.py", "deployment.json"):
        with (workspace / name).open("rb") as stream:
            info = os.fstat(stream.fileno())
            files[name] = {"content": stream.read().decode(), "identity": {
                "device": info.st_dev, "inode": info.st_ino, "mode": stat.S_IMODE(info.st_mode)}}
    return {"files": files, "target": identity(workspace / "shipping.py")}


class DirectoryWatch:
    """Retain workspace events in kernel order from before worker launch through inspection after exit."""

    def __init__(self, workspace):
        """Install a nonblocking inotify watch on the workspace and record when coverage begins."""
        libc = ctypes.CDLL(None, use_errno=True)
        self.fd = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
        if self.fd < 0:
            raise OSError(ctypes.get_errno(), "inotify_init1")
        self.wd = libc.inotify_add_watch(self.fd, os.fsencode(workspace),
                                         0x2 | 0x4 | 0x8 | 0x40 | 0x80 | 0x100 | 0x200 | 0x400 | 0x800)
        if self.wd < 0:
            os.close(self.fd)
            raise OSError(ctypes.get_errno(), "inotify_add_watch")
        self.events = []
        self.armed = time.monotonic_ns()

    def drain(self):
        """Append all queued events with their move cookies. Fail on a lost watch or queue overflow."""
        while True:
            try:
                data = os.read(self.fd, 65536)
            except BlockingIOError:
                return
            while data:
                wd, mask, cookie, size = struct.unpack_from("iIII", data)
                name = os.fsdecode(data[16:16 + size].rstrip(b"\0"))
                if wd != self.wd or mask & (0x4000 | 0x8000):
                    raise AssertionError(("observer lost directory coverage", wd, mask))
                self.events.append(dict(wd=wd, mask=mask, cookie=cookie, name=name))
                data = data[16 + size:]

    def pin_stage(self, stack, f, name):
        """Wait for the stage writer to close, check the proposed bytes and return the open file and its
        event boundary.
        """
        until = time.monotonic() + 4
        while time.monotonic() < until:
            self.drain()
            if any(e["name"] == name and e["mask"] & 0x8 for e in self.events):
                break
            select.select([self.fd], [], [], .01)
        else:
            raise AssertionError(("stage never closed for writing", self.events))
        handle = stack.enter_context((f.workspace / name).open("rb"))
        inode = os.fstat(handle.fileno())
        assert inode.st_nlink == 1 and handle.read().decode() == f.proposal["content"]
        self.drain()
        return handle, len(self.events)


def observe(f, operator, disposition, *, seal_before_stop=False, corroborations=None):
    """Capture real host observations. The supervisor can withhold the single entry permit."""
    with contextlib.ExitStack() as stack:
        watch = DirectoryWatch(f.workspace)
        stack.callback(os.close, watch.fd)
        before = tracked(f.workspace)
        host, worker = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        stack.callback(host.close)
        stack.callback(worker.close)
        host.settimeout(6)
        host.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
        cookie, = struct.unpack("=Q", worker.getsockopt(socket.SOL_SOCKET, 57, 8))  # SO_COOKIE
        operator.config["entry_socket_cookie"] = str(cookie)
        f.slot.configure()
        lease = stack.enter_context(operator.writers.open("rb"))
        fcntl.flock(lease, fcntl.LOCK_SH)
        point = {"replaced": "target.after_rename", "stopped_before_rename": "archive_effect.create",
                 "not_started": None}[disposition]
        if seal_before_stop:
            assert disposition == "stopped_before_rename"
            point = "submit.before_response"
        launched = time.monotonic_ns()
        child = stack.enter_context(f.start(point=point, supervisor_fds=((5, worker.fileno()), (8, lease.fileno()))))
        stack.callback(terminate, child)
        worker.close()
        child.stdin.write(canonical(f.proposal))
        child.stdin.close()
        child.stdin = None
        pidfd = os.pidfd_open(child.pid)
        stack.callback(os.close, pidfd)
        start_ticks = Path(f"/proc/{child.pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
        try:
            data, controls, flags, _ = host.recvmsg(512, socket.CMSG_SPACE(12))
        except (TimeoutError, ConnectionError) as exc:
            if child.poll() is not None:
                raise AssertionError(("entry gate failed", child.stdout.read(), child.stderr.read())) from exc
            raise
        received = time.monotonic_ns()
        assert flags == 0
        creds, = [struct.unpack("3i", body) for level, kind, body in controls
                  if (level, kind) == (socket.SOL_SOCKET, socket.SCM_CREDENTIALS)]
        assert creds[:2] == (child.pid, os.getuid())
        request = data.decode()
        verb, attempt_id, binding = request.split()
        assert verb == "enter"
        journal = json.loads(f.slot.journal.read_bytes())
        attempt = journal["attempts"][-1]
        assert (attempt_id, binding) == (attempt["attempt_id"], attempt["facts"][-1]["binding"])
        assert [fact["tag"] for fact in attempt["facts"]] == ["intent", "dispatch"]
        reply = stage = None
        bound_at = 0
        if disposition != "not_started":
            reply = f"permit {attempt_id} {binding}"
            with f.authority_lock.open("rb") as guard:
                fcntl.flock(guard, fcntl.LOCK_EX)
                host.sendall(reply.encode())
                name = ".gaap-stage-target-" + attempt_id
                held, bound_at = watch.pin_stage(stack, f, name)
                if disposition == "stopped_before_rename":
                    grant = json.loads((f.authority / "authority.json").read_bytes())
                    grant.update(revoked=True, revision=grant["revision"] + 1)
                    nxt = f.authority / "next.json"
                    write_private(nxt, canonical(grant))
                    nxt.replace(f.authority / "authority.json")
                    sync_dir(f.authority)
            if not select.select([child.stderr], [], [], 8)[0]:
                raise AssertionError(("unreached post-effect qualification point", point))
            assert child.stderr.readline().decode().strip() == "journal qualification stop " + point
            _, stopped = os.waitpid(child.pid, os.WUNTRACED)
            assert os.WIFSTOPPED(stopped) and os.WSTOPSIG(stopped) == signal.SIGSTOP
        signal.pidfd_send_signal(pidfd, signal.SIGKILL)
        poll = select.poll()
        poll.register(pidfd, select.POLLIN)
        [(ready, mask)] = poll.poll(5000)
        assert ready == pidfd and mask & select.POLLIN
        wait = os.waitid(os.P_PIDFD, pidfd, os.WEXITED | os.WNOWAIT)
        assert wait.si_pid == child.pid and wait.si_code == os.CLD_KILLED
        assert child.wait(timeout=5) == -signal.SIGKILL
        stopped_ns = time.monotonic_ns()
        watch.drain()
        after = tracked(f.workspace)
        if disposition != "not_started":
            meta = os.fstat(held.fileno())
            held.seek(0)
            stage = dict(name=name, dev=str(meta.st_dev), ino=str(meta.st_ino), uid=meta.st_uid,
                         mode=stat.S_IMODE(meta.st_mode), content=held.read().decode(), final_nlink=meta.st_nlink)
            assert meta.st_nlink == int(disposition == "replaced")
            if disposition == "replaced":
                assert after["target"]["ino"] == str(meta.st_ino)
        if disposition != "replaced":
            assert before == after
        stop = dict(pid=child.pid, boot_id=Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
                    start_ticks=start_ticks, launched_ns=str(launched), stopped_ns=str(stopped_ns),
                    pidfd_revents=mask, wait_code=wait.si_code, wait_status=wait.si_status)
        record = {"format": "gaap.shipping-observer-candidate/0.2",
                  **{k: operator.profile[k] for k in ("workspace_id", "store_id", "profile_digest")},
                  "reconciliation_profile_digest": operator.profile_digest,
                  "observer_id": operator.profile["effect_observer"], "attempt_id": attempt_id, "binding": binding,
                  "workspace": identity(f.workspace), "before": before, "after": after,
                  "watch": dict(wd=watch.wd, armed_ns=str(watch.armed), events=watch.events, bound_at=bound_at,
                                drained_ns=str(time.monotonic_ns())),
                  "gate": dict(request=request, reply=reply, sender_pid=creds[0], sender_uid=creds[1],
                               received_ns=str(received)), "stop": stop, "stage": stage}
        if corroborations is not None:
            # Preserve three separate drains of the still-open observer, for the exact-fit case.
            terminal_events = list(watch.events)
            for _ in range(3):
                watch.drain()
                if watch.events != terminal_events or tracked(f.workspace) != after:
                    raise AssertionError("workspace changed while collecting corroborating observations")
                sample = copy.deepcopy(record)
                sample["watch"]["drained_ns"] = str(time.monotonic_ns())
                corroborations.append(sample)
    operator.config["entry_socket_cookie"] = None
    f.slot.configure()
    return record

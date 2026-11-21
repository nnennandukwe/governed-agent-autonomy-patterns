"""Observe the shipping reference's lock, target rename and restart on private Linux ext4."""

import argparse
import contextlib
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
import subprocess
import sys
import tempfile
import time

from qual_history import GOOD, reference
from qual_host import canonical, digest, observe_tree

SOURCE_SUFFIX = "src/coding/workspace.rs"
SOURCE = Path(__file__).resolve().parents[3] / "examples/submitted-proposal" / SOURCE_SUFFIX
CUTS = ("before-rename", "after-rename")


def source_line(label):
    """Locate the legacy executor's persist call and its successful return in this source revision."""
    sequence = ['.persist(self.path().join("shipping.py"))', '.map_err(|e| e.to_string())?;', 'Ok(())']
    lines = [line.strip() for line in SOURCE.read_text().splitlines()]
    starts = [i for i in range(len(lines)) if lines[i:i + len(sequence)] == sequence]
    if label not in CUTS or len(starts) != 1:
        raise AssertionError(("legacy target replacement changed", label, starts))
    return starts[0] + (1 if label == "before-rename" else len(sequence))


def record_file(path):
    """Read bytes and identity from one open file description."""
    with path.open("rb") as stream:
        info = os.fstat(stream.fileno())
        return {"dev": info.st_dev, "ino": info.st_ino, "digest": digest(stream.read())}


def probe_locks(root, expected):
    """Attempt both nonblocking locks in the cooperating writer's acquisition order."""
    observations = {}
    with contextlib.ExitStack() as stack:
        for name in ("operator/lock", "operator"):
            fd = os.open(root / name, os.O_RDONLY | os.O_NOFOLLOW)
            stack.callback(os.close, fd)
            info = os.fstat(fd)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                status = "acquired"
            except BlockingIOError:
                status = "blocked"
            if status != expected:
                raise AssertionError((name, expected, status))
            observations[name] = [info.st_dev, info.st_ino]
    return observations


@contextlib.contextmanager
def watch_target(workspace):
    """Count kernel move-to events for shipping.py independently of the debugger."""
    libc = ctypes.CDLL(None, use_errno=True)
    fd = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
    if fd < 0:
        raise OSError(ctypes.get_errno(), "inotify_init1")
    try:
        moved_to = 0x80
        if libc.inotify_add_watch(fd, os.fsencode(workspace), moved_to) < 0:
            raise OSError(ctypes.get_errno(), "inotify_add_watch")

        def count():
            """Drain move notifications since the previous observation."""
            events = 0
            while True:
                try:
                    data = os.read(fd, 8192)
                except BlockingIOError:
                    return events
                while data:
                    wd, mask, _, size = struct.unpack_from("iIII", data)
                    if wd < 0 or mask != moved_to:
                        raise AssertionError(("unexpected inotify event", wd, mask))
                    events += data[16:16 + size].rstrip(b"\0") == b"shipping.py"
                    data = data[16 + size:]
        yield count
    finally:
        os.close(fd)


def interrupt_at_source(lldb, debugger, case, binary, line, relay_fd):
    """Stop the writer in LLDB and hand its kernel identity to the outer observer."""
    os.set_inheritable(relay_fd, False)
    root = case / "session"
    debugger.SetAsync(False)
    target = debugger.CreateTarget(str(binary))
    breakpoint = target.BreakpointCreateByLocation(SOURCE_SUFFIX, line)
    if breakpoint.GetNumLocations() < 1:
        raise AssertionError(f"no reference line table at {SOURCE_SUFFIX}:{line}")
    launch = lldb.SBLaunchInfo(["coding-submit", str(root)])
    launch.SetLaunchFlags(launch.GetLaunchFlags() & ~lldb.eLaunchFlagDisableASLR)
    launch.AddOpenFileAction(0, str(case / "proposal.json"), True, False)
    launch.AddOpenFileAction(1, str(case / "child.stdout"), False, True)
    launch.AddOpenFileAction(2, str(case / "child.stderr"), False, True)
    error = lldb.SBError()
    process = target.Launch(launch, error)
    try:
        if not error.Success() or process.GetState() != lldb.eStateStopped:
            raise AssertionError(("reference didn't stop", str(error), process.GetState()))
        thread = next(t for t in process if t.GetStopReason() == lldb.eStopReasonBreakpoint
                      and t.GetStopReasonDataAtIndex(0) == breakpoint.GetID())
        location = thread.GetFrameAtIndex(0).GetLineEntry()
        if location.GetLine() != line or not str(location.GetFileSpec()).endswith(SOURCE_SUFFIX):
            raise AssertionError(("wrong source breakpoint", str(location)))
        pid = process.GetProcessID()
        # No worker process can inherit this invocation's ability to replace the target.
        tasks = list(Path(f"/proc/{pid}/task").iterdir())
        if not tasks or any((task / "children").read_text().strip() for task in tasks):
            raise AssertionError("unexpected subprocess at the target rename")
        pidfd = os.pidfd_open(pid)
        try:
            with socket.socket(fileno=relay_fd) as relay:
                socket.send_fds(relay, [str(pid).encode()], [pidfd])
            locks = probe_locks(root, "blocked")
            stopped_file = record_file(root / "workspace/shipping.py")
            state_path = root / "operator/state.json"
            state = json.loads(state_path.read_bytes())
            if not state["inflight"] or state["runs"]:
                raise AssertionError("expected one unsealed admission before termination")
            stopped_ledger = record_file(state_path)
            signal.pidfd_send_signal(pidfd, signal.SIGKILL)
            if process.GetState() == lldb.eStateStopped:
                process.Continue()
            deadline = time.monotonic() + 5
            poller = select.poll()
            poller.register(pidfd, select.POLLIN)
            if not poller.poll(5000):
                raise AssertionError(("executor didn't exit", process.GetState()))
            while True:
                terminal = process.GetState()
                if terminal == lldb.eStateExited:
                    break
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise AssertionError(("debugger hasn't reported executor exit", terminal))
                time.sleep(min(0.05, remaining))
            result = {"source_file": str(location.GetFileSpec()), "line": line,
                      "breakpoint_id": breakpoint.GetID(), "kernel_tasks": len(tasks),
                      "pid": pid, "signal": "SIGKILL",
                      "debugger_state": lldb.SBDebugger.StateAsCString(terminal),
                      "debugger_exit": process.GetExitStatus(), "exit_description": process.GetExitDescription(),
                      "locks_while_stopped": locks,
                      "stopped_target": stopped_file, "stopped_ledger": stopped_ledger,
                      "inflight": state["inflight"]}
            (case / "cut.json").write_bytes(canonical(result))
        finally:
            os.close(pidfd)
    finally:
        if process.IsValid() and process.GetState() not in (lldb.eStateExited, lldb.eStateDetached):
            process.Kill()


def reap_debugger_children(children):
    """Kill only this supervisor's direct children, including orphans adopted as parents exit."""
    deadline = time.monotonic() + 5
    while True:
        try:
            exited, _ = os.waitpid(-1, os.WNOHANG)
        except ChildProcessError:
            return
        if time.monotonic() >= deadline:
            raise TimeoutError(f"debugger child cleanup timed out: {children.read_text().strip()}")
        if exited:
            continue
        for child in children.read_text().split():
            # This single-threaded parent hasn't reaped this PID since listing it.
            try:
                pidfd = os.pidfd_open(int(child))
                try:
                    signal.pidfd_send_signal(pidfd, signal.SIGKILL)
                finally:
                    os.close(pidfd)
            except ProcessLookupError:
                pass
        time.sleep(0.01)


def supervise_debugger(arguments, trace, relay_fd):
    """Own one debugger tree, including detached descendants that never reached a breakpoint."""
    supervisor = os.fork()
    if supervisor:
        _, status = os.waitpid(supervisor, 0)
        return os.waitstatus_to_exitcode(status)

    result = 1
    try:
        prctl = ctypes.CDLL(None, use_errno=True).prctl
        prctl.argtypes = [ctypes.c_int] + [ctypes.c_ulong] * 4
        if prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
            raise OSError(ctypes.get_errno(), "couldn't own debugger descendants")
        children = Path("/proc/thread-self/children")
        children.read_text()  # Require kernel child enumeration before starting LLDB.
        signal.signal(signal.SIGCHLD, signal.SIG_DFL)
        try:
            child = subprocess.Popen(arguments, stdout=trace, stderr=subprocess.STDOUT, pass_fds=(relay_fd,),
                                     start_new_session=True, env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"})
            result = child.wait(timeout=45)
        finally:
            reap_debugger_children(children)
    except BaseException as error:
        result = 1
        os.write(trace.fileno(), f"debugger supervision failed: {error}\n".encode())
    finally:
        os._exit(0 if result == 0 else 1)


def debugger_cut(case, binary, lldb, line, log_path):
    """Launch only this fixture under LLDB, with a deadline for the complete cut."""
    module_dir = str(Path(__file__).resolve().parent)
    receiver, sender = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
    with receiver, sender, (case / "lldb.log").open("w+b") as trace:
        command = (f"script import sys; sys.path.insert(0, {module_dir!r}); "
                   f"import qual_executor_quiescence as q; q.interrupt_at_source(lldb, lldb.debugger, "
                   f"q.Path({str(case)!r}), q.Path({str(binary)!r}), {line}, {sender.fileno()})")
        arguments = [str(lldb), "--no-lldbinit", "--batch", "-o", command]
        returncode = supervise_debugger(arguments, trace, sender.fileno())
        trace.seek(max(0, trace.seek(0, os.SEEK_END) - 8192))
        log = trace.read(8192)
        log_path.write_bytes(log)
        if returncode or not (case / "cut.json").is_file():
            raise AssertionError(log.decode(errors="replace")[-4000:])
        result = json.loads((case / "cut.json").read_bytes())
        # LLDB can report exit before its parent reaps the task. The supervisor has now
        # drained the debugger tree. Check the same pidfd that signalled the stopped writer.
        receiver.setblocking(False)
        payload, fds, flags, _ = socket.recv_fds(receiver, 32, 1)
        try:
            if len(fds) != 1 or flags & socket.MSG_CTRUNC or payload != str(result["pid"]).encode():
                raise AssertionError("missing or mismatched executor pidfd")
            info = Path(f"/proc/self/fdinfo/{fds[0]}").read_text()
            result["pidfd_final_pid"] = int(dict(row.split(":", 1) for row in info.splitlines())["Pid"])
            if result["pidfd_final_pid"] != -1:
                raise AssertionError(("executor hasn't been reaped", result["pidfd_final_pid"]))
        finally:
            for fd in fds:
                os.close(fd)
        return result


def retained_stage(workspace, initial_entries, before_rename, proposed_digest):
    """Identify the interrupted writer's private stage and check other workspace entries."""
    entries = observe_tree(workspace)
    for name, initial in initial_entries.items():
        if name != "shipping.py" and entries.get(name) != initial:
            raise AssertionError(("unrelated workspace entry changed", name))
    added = entries.keys() - initial_entries.keys()
    if len(added) != int(before_rename):
        raise AssertionError(("unexpected staged entries", sorted(added)))
    if not added:
        return None
    name = added.pop()
    path = workspace / name
    info = path.lstat()
    if (not stat.S_ISREG(info.st_mode) or stat.S_IMODE(info.st_mode) != 0o600
            or info.st_uid != os.geteuid() or info.st_nlink != 1):
        raise AssertionError(("unexpected stage type, permissions or owner", name))
    stage = record_file(path)
    if stage["digest"] != proposed_digest:
        raise AssertionError(("stage differs from proposed bytes", name))
    return {"name": name, **stage}


def qualify_case(label, case, binary, lldb, evidence):
    """Compare the independently observed file and lock with restart and duplicate results."""
    root = case / "session"
    initial = reference(binary, root, "coding-init")
    target = root / "workspace/shipping.py"
    ledger = root / "operator/state.json"
    proposal = {"path": "shipping.py", "content": GOOD, "plan": "Implement the shipping rules.",
                "base_digest": initial["subject_digest"]}
    (case / "proposal.json").write_bytes(canonical(proposal))
    old = record_file(target)
    locks = probe_locks(root, "acquired")
    initial_entries = observe_tree(target.parent)
    with watch_target(target.parent) as moves:
        if label == "completed-control":
            run = reference(binary, root, "coding-submit", proposal)
            if run["receipt"]["body"]["terminal_status"] != "completed":
                raise AssertionError("uninterrupted submission didn't complete")
            result = {"receipt": run["receipt"]["receipt_digest"]}
        else:
            result = debugger_cut(case, binary, lldb, source_line(label),
                                  evidence / f"executor-{label}-lldb.log")
            result["locks_after_death"] = probe_locks(root, "acquired")
            if result["locks_while_stopped"] != locks or result["locks_after_death"] != locks:
                raise AssertionError("writer or snapshot lock inode changed")
            if result["stopped_target"] != record_file(target) or result["stopped_ledger"] != record_file(ledger):
                raise AssertionError("death changed the bytes or identity seen at the breakpoint")
        found = record_file(target)
        replacements = moves()
        if label == "before-rename":
            if old != found or replacements != 0:
                raise AssertionError("target changed before rename")
        elif (old["ino"] == found["ino"] or found["digest"] != digest(GOOD.encode()) or replacements != 1):
            raise AssertionError(("expected a single replacement", old, found, replacements))
        result["retained_stage"] = retained_stage(
            target.parent, initial_entries, label == "before-rename", digest(GOOD.encode()))
        observed = observe_tree(root)
        inspected = reference(binary, root, "coding-inspect")
        result["inspected"] = {"inflight": inspected["inflight"], "published_runs": len(inspected["runs"])}
        if label == "completed-control":
            repeated = reference(binary, root, "coding-submit", proposal)
            if (not repeated["duplicate"] or repeated["receipt"] != run["receipt"]
                    or inspected["runs"] != [run] or inspected["inflight"]):
                raise AssertionError("duplicate or inspector failed to return the original run")
            result["duplicate_receipt"] = repeated["receipt"]["receipt_digest"]
        else:
            refused = subprocess.run([binary, "coding-submit", root], input=canonical(proposal),
                                     capture_output=True, timeout=20)
            if (not refused.returncode or b"no replay dispatched" not in refused.stderr
                    or inspected["runs"] or inspected["inflight"] != result["inflight"]):
                raise AssertionError(("restart failed to retain the unsealed hold", refused.stderr))
            result["duplicate_refusal"] = {"exit": refused.returncode, "reason": refused.stderr.decode().strip()}
        duplicate_moves = moves()
        if duplicate_moves or observe_tree(root) != observed:
            raise AssertionError("inspection or duplicate submission changed the session")
    return {"case": label, "initial_target": old, "final_target": found, "target_moves": replacements,
            "duplicate_target_moves": duplicate_moves, "lock_inodes": locks, **result}


def main():
    """Run the three cases sequentially on an explicitly supplied private ext4 root."""
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("reference", "root", "evidence"):
        parser.add_argument("--" + option, type=Path, required=True)
    parser.add_argument("--lldb", type=Path, default=Path("/usr/lib/llvm-18/bin/lldb"))
    args = parser.parse_args()
    if sys.platform != "linux" or os.geteuid() == 0:
        parser.error("run as an ordinary Linux user")
    if (args.root.stat().st_uid != os.geteuid() or stat.S_IMODE(args.root.stat().st_mode) != 0o700
            or subprocess.check_output(["findmnt", "-n", "-o", "FSTYPE", "-T", args.root],
                                       text=True, timeout=5).strip() != "ext4"):
        parser.error("--root must be an owned private directory on ext4")
    for label in ("completed-control", *CUTS):
        with tempfile.TemporaryDirectory(dir=args.root) as case:
            result = qualify_case(label, Path(case), args.reference.resolve(), args.lldb.resolve(), args.evidence)
        (args.evidence / f"executor-{label}.json").write_bytes(canonical(result) + b"\n")
        print(f"{label} passed", flush=True)


if __name__ == "__main__":
    main()

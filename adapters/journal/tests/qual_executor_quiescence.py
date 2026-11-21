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
import stat
import struct
import subprocess
import sys
import tempfile
import time

from qual_history import GOOD, reference
from qual_host import canonical, digest, observe_tree

SOURCE = Path(__file__).resolve().parents[3] / "examples/submitted-proposal/src/coding/mod.rs"
CUTS = {"before-rename": 'self.store.hit("target_rename")?;',
        "after-rename": 'self.store.hit("postcommit").map_err(|e| (true, e))?;'}


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


def interrupt_at_source(lldb, debugger, case, binary, line):
    """Run inside LLDB's native interpreter and retain evidence only after the child is reaped."""
    root = case / "session"
    debugger.SetAsync(False)
    target = debugger.CreateTarget(str(binary))
    breakpoint = target.BreakpointCreateByLocation("src/coding/mod.rs", line)
    if breakpoint.GetNumLocations() < 1:
        raise AssertionError(f"no reference line table at src/coding/mod.rs:{line}")
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
        if location.GetLine() != line or not str(location.GetFileSpec()).endswith("src/coding/mod.rs"):
            raise AssertionError(("wrong source breakpoint", str(location)))
        pid = process.GetProcessID()
        # No worker process can inherit this invocation's ability to replace the target.
        tasks = list(Path(f"/proc/{pid}/task").iterdir())
        if not tasks or any((task / "children").read_text().strip() for task in tasks):
            raise AssertionError("unexpected subprocess at the target rename")
        pidfd = os.pidfd_open(pid)
        try:
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
            # The pidfd stays bound to this invocation. Linux prints -1 once its task is reaped.
            info = Path(f"/proc/self/fdinfo/{pidfd}")
            while True:
                fields = dict(row.split(":", 1) for row in info.read_text().splitlines())
                bound_pid = int(fields["Pid"])
                terminal = process.GetState()
                if bound_pid == -1 and terminal == lldb.eStateExited:
                    break
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise AssertionError(("executor hasn't been reaped", bound_pid, terminal))
                time.sleep(min(0.05, remaining))
            result = {"line": line, "breakpoint_id": breakpoint.GetID(), "kernel_tasks": len(tasks),
                      "pid": pid, "signal": "SIGKILL", "pidfd_final_pid": bound_pid,
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


def debugger_cut(case, binary, lldb, line, log_path):
    """Launch only this fixture under LLDB, with a deadline for the complete cut."""
    module_dir = str(Path(__file__).resolve().parent)
    command = (f"script import sys; sys.path.insert(0, {module_dir!r}); "
               f"import qual_executor_quiescence as q; q.interrupt_at_source(lldb, lldb.debugger, "
               f"q.Path({str(case)!r}), q.Path({str(binary)!r}), {line})")
    with (case / "lldb.log").open("w+b") as trace:
        with subprocess.Popen([str(lldb), "--no-lldbinit", "--batch", "-o", command],
                              stdout=trace, stderr=subprocess.STDOUT, start_new_session=True,
                              env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}) as child:
            try:
                child.wait(timeout=45)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=5)
                raise
            finally:
                trace.seek(max(0, trace.seek(0, os.SEEK_END) - 8192))
                log = trace.read(8192)
                log_path.write_bytes(log)
    if child.returncode or not (case / "cut.json").is_file():
        raise AssertionError(log.decode(errors="replace")[-4000:])
    return json.loads((case / "cut.json").read_bytes())


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
    with watch_target(target.parent) as moves:
        if label == "completed-control":
            run = reference(binary, root, "coding-submit", proposal)
            if run["receipt"]["body"]["terminal_status"] != "completed":
                raise AssertionError("uninterrupted submission didn't complete")
            result = {"receipt": run["receipt"]["receipt_digest"]}
        else:
            matches = [i for i, text in enumerate(SOURCE.read_text().splitlines(), 1)
                       if text.strip() == CUTS[label]]
            if len(matches) != 1:
                raise AssertionError(("target source marker changed", label, matches))
            result = debugger_cut(case, binary, lldb, matches[0], evidence / f"executor-{label}-lldb.log")
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

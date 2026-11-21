"""Exercise the Linux journal CLI on an explicitly qualified disposable ext4 mount."""

import argparse
import fcntl
import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import unittest

from qual_host import LIMITS, Slot, canonical, digest, identity, observe_tree, snapshot, write_private


class JournalQualification(unittest.TestCase):
    """Every check operates on its own externally provisioned slot."""

    def setUp(self):
        """Give each test a private ext4 root and a separate health directory."""
        self.root_temp = tempfile.TemporaryDirectory(dir=SETTINGS.root)
        self.health_temp = tempfile.TemporaryDirectory(dir=SETTINGS.health_root)
        self.addCleanup(self.root_temp.cleanup)
        self.addCleanup(self.health_temp.cleanup)
        self.root = Path(self.root_temp.name)
        self.health = Path(self.health_temp.name)

    def slot(self, **kwargs):
        """Provision an independent store for one fault or recovery scenario."""
        base = Path(tempfile.mkdtemp(dir=self.root))
        return Slot(base, self.health, SETTINGS.release, SETTINGS.qualification, **kwargs)

    def record(self, slot, case, result):
        """Keep hashed file identities, phases and actual commands' outcomes per cut."""
        row = {"case": case, "result": result, "health": slot.health.read_text(),
               "anchor": snapshot(slot.anchor), "journal": snapshot(slot.journal),
               "workspace": observe_tree(slot.workspace)}
        if slot.anchor.exists():
            row["phase"] = json.loads(slot.anchor.read_bytes())["phase"]
        if slot.journal.exists():
            row["journal_bytes"] = slot.journal.stat().st_size
        with (SETTINGS.evidence / "cases.ndjson").open("a") as report:
            report.write(json.dumps(row, sort_keys=True) + "\n")

    def test_enroll_restart_trace_and_read_only_lookup(self):
        """Trace the three publications and recheck the original bytes after repeat enrollment."""
        slot = self.slot()
        slot.call("lookup", "never", expected="not_enrolled")
        previous = observe_tree(slot.workspace)
        trace = SETTINGS.evidence / "enroll-syscalls.log"
        reply = slot.call("enroll", expected="enrolled", trace=trace)
        self.assertEqual(reply["generation"], 0)
        self.assertEqual(json.loads(slot.journal.read_bytes())["attempts"], [])
        journal = snapshot(slot.journal)
        anchor = snapshot(slot.anchor)
        self.assertEqual(json.loads(slot.anchor.read_bytes())["journal_digest"], journal[1])
        self.assertEqual(slot.call("lookup", "never", expected="absent")["generation"], 0)
        slot.call("enroll", expected="enrolled")
        self.assertEqual((snapshot(slot.journal), snapshot(slot.anchor)), (journal, anchor))
        self.assertEqual(observe_tree(slot.workspace), previous)
        # See the successful Linux calls and descriptors from outside the writer.
        calls = trace.read_text().splitlines()
        index = 0
        for stage, directory in [("reserve", slot.control), ("journal", slot.store),
                                 ("activate", slot.control)]:
            for name, target in [("fsync(", ".gaap-stage-" + stage),
                                 ("rename", ".gaap-stage-" + stage),
                                 ("fsync(", str(directory) + ">")]:
                while index < len(calls) and not (
                    name in calls[index] and target in calls[index] and "= 0" in calls[index]
                ):
                    index += 1
                self.assertLess(index, len(calls), (name, target, str(trace)))
                index += 1
        self.record(slot, "enroll-repeat-strace", reply)

    def test_host_descriptor_access_and_read_only_lookup(self):
        """Reject invalid host descriptors and inspect the production reader's file opens."""
        slot = self.slot()
        original = snapshot(slot.anchor)
        for handoff in ("writable-config", "readonly-health", "missing-health"):
            with self.subTest(handoff=handoff):
                slot.call("enroll", expected="unavailable", handoff=handoff)
                self.assertEqual(snapshot(slot.anchor), original)
                self.assertFalse(slot.journal.exists())
                self.assertEqual(slot.health.read_bytes(), b"0")
        slot.call("enroll", expected="enrolled")
        state = (slot.lock, slot.journal, slot.anchor, slot.health)
        before = [(snapshot(path), path.stat().st_ctime_ns) for path in state]
        trace = SETTINGS.evidence / "lookup-syscalls.log"
        slot.call("lookup", "never", expected="absent", trace=trace)
        self.assertEqual([(snapshot(path), path.stat().st_ctime_ns) for path in state], before)
        calls = trace.read_text().splitlines()
        for path in (slot.lock, slot.anchor, slot.journal):
            # strace -yy prints the resolved file of each returned descriptor.
            opened = [line for line in calls if f"<{path}>" in line.rsplit(" = ", 1)[-1]
                      and ("openat(" in line or "openat2(" in line)]
            self.assertTrue(opened, ("missing open of", path))
            self.assertTrue(all("O_RDONLY" in line and "O_RDWR" not in line
                                and "O_WRONLY" not in line and "O_TRUNC" not in line for line in opened))
        for call in ("rename(", "renameat(", "renameat2(", "unlinkat(", "linkat(", "ftruncate(", "pwrite64("):
            self.assertFalse(any(call in line for line in calls), ("unexpected historical lookup mutation", call))

    def test_process_interruption_at_each_commit(self):
        """Stop and kill the writer around each rename and sync, then read the surviving state."""
        stages = ("reserve", "journal", "activate")
        operations = ("before_rename", "after_rename", "before_dir_sync", "after_dir_sync")
        points = [stage + "." + operation for stage in stages for operation in operations]
        for point in points + ["journal.reread", "enroll.before_response"]:
            with self.subTest(point=point):
                slot = self.slot()
                workspace = observe_tree(slot.workspace)
                before = snapshot(slot.anchor)
                writer = slot.stop(point)
                writer.kill()
                out, _ = writer.communicate(timeout=5)
                self.assertEqual(writer.returncode, -signal.SIGKILL)
                self.assertEqual(out, b"", "killed client received an acknowledgment")
                self.assertEqual(slot.health.read_bytes(), b"0", "healthy boundary held a pending operation")
                if point == "reserve.before_rename":
                    status = "not_enrolled"
                    self.assertEqual(snapshot(slot.anchor), before)
                elif point in ("activate.after_rename", "activate.before_dir_sync",
                               "activate.after_dir_sync", "enroll.before_response"):
                    status = "absent"
                else:
                    status = "incomplete_enrollment"
                trace = slot.call("lookup", "never", expected=status)
                if status == "absent":
                    visible = snapshot(slot.journal)
                    slot.call("enroll", expected="enrolled")
                    self.assertEqual(snapshot(slot.journal), visible)
                elif status == "incomplete_enrollment":
                    entries = observe_tree(slot.store), observe_tree(slot.control)
                    slot.call("enroll", expected=status)
                    self.assertEqual((observe_tree(slot.store), observe_tree(slot.control)), entries)
                self.assertEqual(observe_tree(slot.workspace), workspace)
                self.record(slot, point, trace)

    def activation_sync_ordinal(self, baseline):
        """Locate the active anchor's directory sync in a successful production enrollment."""
        healthy = self.slot()
        healthy.call("enroll", expected="enrolled", trace=baseline)
        syncs = [line for line in baseline.read_text().splitlines() if "fsync(" in line]
        control_syncs = [i for i, line in enumerate(syncs, 1) if f"<{healthy.control}>" in line]
        self.assertEqual(len(control_syncs), 2, syncs)
        return control_syncs[-1]

    def test_process_dies_after_real_sync_error_before_health_update(self):
        """Kill after a directory sync returns EIO and confirm both restarts retain the hold."""
        ordinal = self.activation_sync_ordinal(SETTINGS.evidence / "sync-calibration.log")
        slot = self.slot()
        workspace = observe_tree(slot.workspace)
        trace = SETTINGS.evidence / "eio-before-quarantine.log"
        writer = slot.stop("volume.after_eio", trace=trace, fail_sync_ordinal=ordinal)
        try:
            self.assertTrue(any(f"<{slot.control}>" in line and "fsync(" in line
                                and "EIO" in line and "INJECTED" in line
                                for line in trace.read_text().splitlines()))
            self.assertEqual(slot.health.read_bytes(), b"P")
            active = json.loads(slot.anchor.read_bytes())
            self.assertEqual(active["phase"], "active")
            self.assertEqual(active["generation"], 0)
            self.assertEqual(active["journal_digest"], digest(slot.journal.read_bytes()))
            before = (snapshot(slot.anchor), snapshot(slot.journal), snapshot(slot.health))
            writer.kill()
            out, _ = writer.communicate(timeout=5)
            self.assertEqual(writer.returncode, -signal.SIGKILL)
            self.assertEqual(out, b"")
        finally:
            if writer.poll() is None:
                writer.kill()
                writer.communicate()
        reads = slot.call("lookup", "never", expected="unavailable")
        retry = slot.call("enroll", expected="unavailable")
        self.assertNotIn("attempt", reads)
        self.assertEqual(slot.health.read_bytes(), b"P")
        self.assertEqual((snapshot(slot.anchor), snapshot(slot.journal), snapshot(slot.health)), before)
        self.assertEqual(observe_tree(slot.workspace), workspace)
        self.record(slot, "killed-after-injected-eio", {"lookup": reads, "retry": retry})

    def test_failed_writeback_at_activation_holds_store(self):
        """Keep the visible generation on hold when its directory sync fails."""
        ordinal = self.activation_sync_ordinal(SETTINGS.evidence / "activation-sync-calibration.log")
        for errno in ("ENOSPC", "EDQUOT", "EROFS"):
            with self.subTest(errno=errno):
                slot = self.slot()
                trace = SETTINGS.evidence / f"activation-sync-{errno.lower()}.log"
                workspace = observe_tree(slot.workspace)
                failure = slot.call("enroll", expected=("unavailable", "capacity"), trace=trace,
                                    fail_sync_ordinal=ordinal, sync_errno=errno)
                self.assertTrue(any(f"<{slot.control}>" in line and "fsync(" in line
                                    and errno in line and "INJECTED" in line
                                    for line in trace.read_text().splitlines()))
                anchor = json.loads(slot.anchor.read_bytes())
                self.assertEqual(anchor["phase"], "active")
                self.assertEqual(anchor["journal_digest"], digest(slot.journal.read_bytes()))
                visible = snapshot(slot.anchor), snapshot(slot.journal)
                latch = slot.health.read_bytes()
                # Record the complete restart behavior even if the original process missed the fault.
                lookup = slot.call("lookup", "never", expected=("unavailable", "absent"))
                retry = slot.call("enroll", expected=("unavailable", "enrolled"))
                self.record(slot, "delayed-activation-" + errno,
                            {"error": failure, "lookup": lookup, "retry": retry})
                self.assertEqual((failure["status"], latch, lookup["status"], retry["status"]),
                                 ("unavailable", b"1", "unavailable", "unavailable"))
                self.assertEqual(slot.health.read_bytes(), b"1")
                self.assertEqual((snapshot(slot.anchor), snapshot(slot.journal)), visible)
                self.assertEqual(observe_tree(slot.workspace), workspace)

    def test_journal_read_eio_quarantines_across_restart(self):
        """Inject EIO on the actual journal read and retain evidence and the independent hold."""
        slot = self.slot()
        slot.call("enroll", expected="enrolled")
        before = snapshot(slot.journal), snapshot(slot.anchor), observe_tree(slot.workspace)
        baseline = SETTINGS.evidence / "read-calibration.log"
        slot.call("lookup", "never", expected="absent", read_trace=baseline)
        reads = [line for line in baseline.read_text().splitlines() if line.startswith("read(")]
        journal_reads = [ordinal for ordinal, line in enumerate(reads, 1) if f"<{slot.journal}>" in line]
        self.assertTrue(journal_reads, "release reader didn't read the enrolled journal")
        injected = SETTINGS.evidence / "journal-read-eio.log"
        failure = slot.call("lookup", "never", expected="unavailable", read_trace=injected,
                            fail_read_ordinal=journal_reads[0])
        self.assertTrue(any(line.startswith("read(") and f"<{slot.journal}>" in line
                            and "EIO" in line and "INJECTED" in line
                            for line in injected.read_text().splitlines()))
        self.assertEqual(slot.health.read_bytes(), b"1")
        fresh = slot.call("lookup", "never", expected="unavailable")
        slot.call("enroll", expected="unavailable")
        self.assertEqual((snapshot(slot.journal), snapshot(slot.anchor), observe_tree(slot.workspace)), before)
        self.record(slot, "journal-read-eio", {"failure": failure, "restarted": fresh})

    def test_failed_storage_operations_preserve_bytes_and_quarantine(self):
        """Inject each storage failure and compare surviving bytes with its expected commit phase."""
        cases = [(op, "fail") for op in ("create", "write", "flush", "close", "before_rename")]
        cases += [("write", "short"), ("write", "enospc"),
                  ("file_sync", "eio"), ("before_dir_sync", "eio")]
        for stage in ("reserve", "journal", "activate"):
            for op, fault in cases:
                point = stage + "." + op
                with self.subTest(point=point, fault=fault):
                    slot = self.slot()
                    workspace = observe_tree(slot.workspace)
                    reply = slot.call("enroll", expected=("capacity", "unavailable", "invalid_store"),
                                      point=point, action=fault)
                    original = json.loads(slot.anchor.read_bytes())
                    if stage == "reserve":
                        self.assertEqual(original["phase"], "reserved" if op == "before_dir_sync" else "unused")
                    elif stage == "journal":
                        self.assertEqual(original["phase"], "reserved")
                    else:
                        self.assertEqual(original["phase"], "active" if op == "before_dir_sync" else "reserved")
                    expected_anchor = {**slot.unused, "phase": original["phase"]}
                    journal_visible = stage == "activate" or stage == "journal" and op == "before_dir_sync"
                    self.assertEqual(slot.journal.exists(), journal_visible)
                    if journal_visible:
                        expected_journal = {field: slot.unused[field] for field in
                                            ("store_id", "workspace_id", "profile_digest")}
                        expected_journal.update(format="gaap.journal/0.1", generation=0, attempts=[])
                        self.assertEqual(slot.journal.read_bytes(), canonical(expected_journal))
                        if original["phase"] == "active":
                            expected_anchor.update(generation=0, journal_digest=digest(canonical(expected_journal)))
                    self.assertEqual(slot.anchor.read_bytes(), canonical(expected_anchor))
                    if fault == "eio":
                        self.assertEqual(slot.health.read_bytes(), b"1")
                        slot.call("lookup", "never", expected="unavailable")
                        slot.call("enroll", expected="unavailable")
                    if fault == "enospc":
                        self.assertEqual(reply["status"], "capacity")
                        self.assertEqual(slot.health.read_bytes(), b"0")
                    self.assertEqual(observe_tree(slot.workspace), workspace)
                    self.record(slot, point + ":" + fault, reply)

    def test_observed_lock_ownership_and_concurrent_enrollment(self):
        """Verify that competing writers wait while shared readers can inspect together."""
        slot = self.slot()
        original = snapshot(slot.lock)
        writer = slot.stop("slot.after_lock")
        try:
            slot.call("enroll", expected="busy")
            slot.call("lookup", "never", expected="busy")
            os.kill(writer.pid, signal.SIGCONT)
            data, err = writer.communicate(timeout=8)
            self.assertEqual(writer.returncode, 0, err)
            self.assertEqual(json.loads(data)["status"], "enrolled")
        finally:
            if writer.poll() is None:
                writer.kill()
                writer.communicate()
        with slot.lock.open("rb") as reader:
            fcntl.flock(reader, fcntl.LOCK_SH)
            slot.call("lookup", "never", expected="absent")
            slot.call("enroll", expected="busy")
        visible = snapshot(slot.journal)
        slot.call("enroll", expected="enrolled")
        self.assertEqual((snapshot(slot.lock), snapshot(slot.journal)), (original, visible))
        self.record(slot, "concurrent-enrollment-shared-read", {"status": "enrolled"})

    def test_colliding_entry_is_preserved(self):
        """Check that each publication refuses to replace a separately owned entry."""
        for cut, name in [("reserve", ".gaap-stage-reserve"),
                          ("journal", "journal.json"), ("activate", ".gaap-stage-activate")]:
            with self.subTest(cut=cut):
                slot = self.slot()
                directory = slot.store if cut == "journal" else slot.control
                point = "journal.before_rename" if cut == "journal" else "slot.after_lock"
                writer = slot.stop(point)
                foreign = directory / name
                try:
                    write_private(foreign, b"separately owned file\n")
                    original = snapshot(foreign)
                    os.kill(writer.pid, signal.SIGCONT)
                    data, err = writer.communicate(timeout=8)
                    self.assertNotEqual(writer.returncode, 0, data)
                    self.assertEqual(snapshot(foreign), original)
                    self.record(slot, "foreign-" + cut, json.loads(data or err))
                finally:
                    if writer.poll() is None:
                        writer.kill()
                        writer.communicate()

    def test_corrupt_incompatible_and_unauthorized_reads(self):
        """Refuse untrusted history and preserve the bytes reached by each read."""
        slot = self.slot()
        slot.call("enroll", expected="enrolled")
        original_journal = json.loads(slot.journal.read_bytes())
        initial = observe_tree(slot.workspace)
        slot.host["can_read"] = False
        slot.configure()
        denied = slot.call("lookup", "existing", expected="forbidden")
        self.assertEqual(slot.call("lookup", "missing", expected="forbidden"), denied)
        slot.host["can_read"] = True
        slot.configure()
        for case, data, status in [
            ("newer", canonical({**original_journal, "generation": 1}), "unconfirmed"),
            ("gap", canonical({**original_journal, "generation": 2}), "invalid_store"),
            ("different-id", canonical({**original_journal, "store_id": "a" * 32}), "invalid_store"),
            ("malformed", b"{", "invalid_store"),
        ]:
            with self.subTest(case=case):
                write_private(slot.journal, data)
                prior = snapshot(slot.journal)
                reply = slot.call("lookup", "missing", expected=status)
                self.assertEqual(snapshot(slot.journal), prior)
                if case != "malformed":
                    self.record(slot, case, reply)
        slot.journal.unlink()
        slot.call("lookup", "never", expected="invalid_store")
        slot.call("enroll", expected="invalid_store")
        self.assertFalse(slot.journal.exists())
        self.assertEqual(observe_tree(slot.workspace), initial)

    def test_boundaries_unknown_format_and_surviving_scratch(self):
        """Check inclusive limits, unknown formats and the diagnostics for failed scratch cleanup."""
        slot = self.slot()
        slot.call("enroll", expected="enrolled")
        for path, limit in [(slot.journal, LIMITS["journal_bytes"]), (slot.anchor, LIMITS["anchor_bytes"])]:
            with self.subTest(path=path.name):
                original = path.read_bytes()
                exact = b'{"extra":"' + b"x" * (limit - len(b'{"extra":""}')) + b'"}'
                self.assertEqual(len(exact), limit)
                write_private(path, exact)
                at_limit = slot.call("lookup", "never", expected="invalid_store")
                write_private(path, exact + b" ")
                over = slot.call("lookup", "never", expected="invalid_store")
                self.assertNotEqual(at_limit["reason"], over["reason"])
                self.assertEqual(path.stat().st_size, limit + 1)
                value = json.loads(original)
                value["format"] = "future/9"
                write_private(path, canonical(value))
                slot.call("lookup", "never", expected="unsupported_storage")
                write_private(path, original)
        other = self.slot()
        ordinary_failure = other.call("enroll", expected="capacity", point="reserve.write", action="enospc")
        self.assertFalse((other.control / ".gaap-stage-reserve").exists())
        reply = other.call("enroll", expected="capacity", point="reserve.write",
                           action="enospc", fail_cleanup=True)
        self.assertEqual(reply["reason"], ordinary_failure["reason"])
        self.assertEqual(reply["warning"], "owned_stage_cleanup_failed")
        self.assertEqual(other.anchor.read_bytes(), canonical(other.unused))
        self.assertFalse(other.journal.exists())
        scratch = other.control / ".gaap-stage-reserve"
        self.assertTrue(scratch.is_file())
        original = snapshot(scratch)
        other.call("enroll", expected="invalid_store")
        self.assertEqual(snapshot(scratch), original)
        self.record(other, "failed-owned-stage-cleanup", reply)

    def test_mount_pin_links_and_unsafe_permissions(self):
        """Refuse a mismatched mount, a linked journal and unsafe read permissions."""
        slot = self.slot()
        slot.host["slot"]["mount_id"] += 1
        slot.configure()
        prior = snapshot(slot.anchor)
        slot.call("enroll", expected="unsupported_storage")
        self.assertEqual(snapshot(slot.anchor), prior)
        slot.host["slot"]["mount_id"] -= 1
        slot.configure()
        slot.call("enroll", expected="enrolled")
        retained = slot.root / "saved-journal"
        slot.journal.rename(retained)
        slot.journal.symlink_to(retained)
        original = snapshot(retained)
        slot.call("lookup", "never", expected="invalid_store")
        self.assertTrue(slot.journal.is_symlink())
        slot.journal.unlink()
        retained.rename(slot.journal)
        slot.journal.chmod(0o644)
        slot.call("lookup", "never", expected="invalid_store")
        self.assertEqual(snapshot(slot.journal)[1], original[1])

    def test_anchor_and_lock_replacement_prevent_publication(self):
        """Replace a pinned entry while the writer is stopped and check that publication fails."""
        for point, pinned in [("slot.before_lock", "lock"), ("reserve.before_rename", "anchor")]:
            with self.subTest(point=point):
                slot = self.slot()
                writer = slot.stop(point)
                path = getattr(slot, pinned)
                replacement = path.with_suffix(".replacement")
                try:
                    write_private(replacement, path.read_bytes())
                    replacement.replace(path)
                    observed = snapshot(path)
                    os.kill(writer.pid, signal.SIGCONT)
                    out, err = writer.communicate(timeout=8)
                    self.assertNotEqual(writer.returncode, 0)
                    reply = json.loads(out or err)
                    self.assertEqual(reply["status"], "invalid_store")
                    self.assertEqual(snapshot(path), observed)
                    self.assertFalse(slot.journal.exists())
                finally:
                    if writer.poll() is None:
                        writer.kill()
                        writer.communicate()

    def test_denied_enrollment_and_untrusted_lock(self):
        """Preserve the unused anchor when enrollment lacks authority or the lock has another link."""
        slot = self.slot()
        slot.host["can_enroll"] = False
        slot.configure()
        previous = snapshot(slot.anchor)
        slot.call("enroll", expected="forbidden")
        self.assertEqual(snapshot(slot.anchor), previous)
        self.assertFalse(slot.journal.exists())
        slot.host["can_enroll"] = True
        slot.configure()
        alias = slot.control / "alias"
        os.link(slot.lock, alias)
        slot.call("enroll", expected="invalid_store")
        self.assertEqual(snapshot(slot.anchor), previous)
        self.assertEqual(identity(alias)["ino"], identity(slot.lock)["ino"])

    def test_sealed_unknown_result_holds_workspace(self):
        """Check that an unresolved effect still holds the workspace after the run receipt is sealed."""
        from qual_history import exercise_unknown
        exercise_unknown(self, SETTINGS.reference, SETTINGS.evidence)

    def test_populated_history_from_reference_execution(self):
        """Return original receipts across restart for completed, failed and blocked reference runs."""
        from qual_history import exercise_histories
        exercise_histories(self, SETTINGS.reference, SETTINGS.evidence)


def main():
    """Require explicit binaries, mount, health authority and evidence output paths."""
    global SETTINGS
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("release", "qualification", "reference", "root", "health-root", "evidence"):
        parser.add_argument("--" + name, required=True, type=lambda p: Path(p).resolve(strict=True))
    SETTINGS = parser.parse_args()
    if os.getuid() == 0:
        parser.error("run the process tests under the ordinary service account")
    os.umask(0o077)
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(JournalQualification)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main())

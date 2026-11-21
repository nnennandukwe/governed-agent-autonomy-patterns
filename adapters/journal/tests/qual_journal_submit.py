"""Qualify the shipping executor against an enrolled journal on a private Linux ext4 mount."""

import argparse
import ctypes
import json
import os
from pathlib import Path
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

from qual_executor_quiescence import reap_debugger_children, record_file, watch_target
from qual_history import GOOD, reference, reference_profile
from qual_host import canonical, digest, observe_tree, snapshot, write_private
from qual_submit_host import Shipping, lock_blocked, subject, terminate, update_grant, watch_publications


def facts(read):
    return {f["tag"]: f["payload"] for f in read["attempt"]["facts"]}


class JournalSubmit(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        """Get shipping policy and artifact bytes from a separate unmodified production reference."""
        with tempfile.TemporaryDirectory(dir=ARGS.root) as work:
            legacy = Path(work) / "legacy"
            initial = reference(ARGS.reference, legacy, "coding-init")
            cls.inputs = {p.name: p.read_bytes() for p in (legacy / "workspace").iterdir()}
            run = reference(ARGS.reference, legacy, "coding-submit", {
                "path": "shipping.py", "content": GOOD, "plan": "Implement shipping rules.",
                "base_digest": initial["subject_digest"]})
            cls.profile, cls.budget = reference_profile(run), run["request"]["resource_budget"]

    def fixture(self, content=GOOD, label=None):
        workspace = tempfile.TemporaryDirectory(dir=ARGS.root)
        health = tempfile.TemporaryDirectory(dir=ARGS.health_root)
        self.addCleanup(workspace.cleanup)
        self.addCleanup(health.cleanup)
        f = Shipping(Path(workspace.name), Path(health.name), ARGS, self.inputs,
                     self.profile, self.budget, content, "shipping-1")
        # Preserve the exact last journal, archive and target identity even if an assertion fails.
        name = self._testMethodName + ("-" + label if label else "")
        case = ARGS.evidence / "submission" / name
        case.mkdir(parents=True)
        self.addCleanup(self.save, f, case)
        return f

    @staticmethod
    def save(f, case):
        for src in (f.slot.anchor, f.slot.journal, f.slot.health):
            if src.exists():
                shutil.copyfile(src, case / ("health" if src == f.slot.health else src.name))
        (case / "outside.json").write_bytes(canonical({"initial": f.initial, "target": record_file(f.target),
            "workspace": observe_tree(f.workspace), "control": observe_tree(f.slot.control),
            "store": observe_tree(f.slot.store), "archive": observe_tree(f.archive)}))
        shutil.copytree(f.archive, case / "archive")

    def reply(self, f, status, **options):
        exit_code, read = f.submit(**options)
        self.assertEqual(read.get("status"), status, read)
        if status in ("published", "partial", "conflict"):
            self.assertEqual(exit_code, 0, read)
        else:
            self.assertNotEqual(exit_code, 0, read)
            self.assertTrue(read.get("reason"), read)
        return read

    def test_sealed_success_current_rights_and_archive(self):
        f = self.fixture()
        initial = snapshot(f.target)
        with watch_target(f.workspace) as moves:
            sent = self.reply(f, "published")
            stored = f.slot.call("lookup", f.key, expected="published")
            self.assertEqual(sent["attempt"], stored["attempt"])
            row = facts(stored)
            self.assertEqual(list(row), ["intent", "dispatch", "observed", "verified", "published"])
            self.assertEqual(row["observed"]["disposition"], "replaced")
            self.assertEqual(row["verified"]["event"]["verdict"], "pass")
            self.assertEqual(row["published"]["receipt"]["body"]["terminal_status"], "completed")
            self.assertIn(f.grant["approval"], stored["attempt"]["run_request"]["approval_context"])
            self.assertEqual(row["observed"]["after"]["subject"]["digest"], subject(f.workspace))
            self.assertEqual(moves(), 1)
            self.assertNotEqual(snapshot(f.target)[0]["ino"], initial[0]["ino"])
            self.assertEqual(f.target.read_bytes(), GOOD.encode())
            before = snapshot(f.slot.journal), snapshot(f.slot.anchor), observe_tree(f.archive)
            update_grant(f.authority, revoked=True)
            f.slot.host["live"]["can_submit"] = False
            f.slot.configure()
            self.assertEqual(self.reply(f, "published", stdin=json.dumps(f.proposal, indent=2).encode())["attempt"],
                             stored["attempt"])
            self.assertNotIn("attempt", self.reply(f, "conflict", stdin=canonical({**f.proposal, "plan": "Changed"})))
            code, rejected = f.submit(key="new-key")
            self.assertNotEqual(code, 0, rejected)
            self.assertNotIn("attempt", rejected)
            f.slot.host["principal"] = "bob"
            f.slot.configure()
            f.slot.call("lookup", f.key, expected="absent")
            f.slot.host["can_read"] = False
            f.slot.configure()
            for key in (f.key, "never"):
                self.assertNotIn("attempt", self.reply(f, "forbidden", key=key))
            self.assertEqual((snapshot(f.slot.journal), snapshot(f.slot.anchor), observe_tree(f.archive)), before)
            self.assertEqual(moves(), 0)
        # A new authorized reader must depend on the original authenticated capsule bytes.
        f.slot.host.update(principal="alice", can_read=True)
        f.slot.configure()
        archived, = f.archive.glob("*.effect.*.json")
        raw = archived.read_bytes()
        capsule = json.loads(raw)
        witness = capsule["source"]["observation"]
        stage = witness["stage"]
        self.assertEqual(int(stage["ino"]), record_file(f.target)["ino"])
        self.assertEqual(int(stage["dev"]), record_file(f.target)["dev"])
        self.assertEqual(witness["binding"], stored["attempt"]["facts"][0]["binding"])
        events = witness["events"][witness["bound_at"]:]
        from_cookie, = [e["cookie"] for e in events if e["name"] == stage["name"] and e["mask"] & 0x40]
        to_cookie, = [e["cookie"] for e in events if e["name"] == "shipping.py" and e["mask"] & 0x80]
        self.assertGreater(from_cookie, 0)
        self.assertEqual(from_cookie, to_cookie)
        self.assertEqual(capsule["source"]["authority"]["revision"], f.grant["revision"])
        self.assertEqual(archived.name.split(".")[-2], digest(raw).split(":")[1])
        archived.unlink()
        f.slot.call("lookup", f.key, expected="unavailable")
        write_private(archived, raw + b" ")
        f.slot.call("lookup", f.key, expected=("invalid_store", "unavailable"))
        write_private(archived, raw)
        self.assertEqual(f.slot.call("lookup", f.key, expected="published")["attempt"], stored["attempt"])
        legacy = subprocess.run([ARGS.reference, "coding-submit", f.root], input=canonical(f.proposal),
                                capture_output=True, timeout=10)
        self.assertNotEqual(legacy.returncode, 0)
        self.assertEqual(snapshot(f.target)[1], digest(GOOD.encode()))

    def test_admission_refusal_and_legacy_workspace(self):
        f = self.fixture()
        base = snapshot(f.slot.anchor), snapshot(f.slot.journal)
        with watch_target(f.workspace) as moves:
            for field, denied, allowed in (("revoked", True, False), ("allow_retention", False, True)):
                update_grant(f.authority, **{field: denied})
                self.reply(f, "forbidden")
                self.assertEqual(f.slot.call("lookup", f.key, expected="absent")["generation"], 0)
                self.assertEqual((snapshot(f.slot.anchor), snapshot(f.slot.journal)), base)
                update_grant(f.authority, **{field: allowed})
            (f.root / "operator").mkdir(mode=0o700)
            self.assertEqual(self.reply(f, "forbidden")["reason"], "legacy_session_workspace")
            self.assertEqual((snapshot(f.slot.anchor), snapshot(f.slot.journal)), base)
            self.assertEqual(moves(), 0)
        f = self.fixture(label="policy")
        f.proposal.update(path="deployment.json", content="{}")
        update_grant(f.authority, proposal_digest=digest(canonical(f.proposal)))
        before = observe_tree(f.workspace)
        with watch_target(f.workspace) as moves:
            saved = self.reply(f, "published")
            self.assertEqual(saved["attempt"], f.slot.call("lookup", f.key, expected="published")["attempt"])
            row = facts(saved)
            self.assertEqual(list(row), ["intent", "published"])
            self.assertNotEqual(row["published"]["receipt"]["body"]["terminal_status"], "completed")
            self.assertEqual(self.reply(f, "published")["attempt"], saved["attempt"])
            self.assertEqual(observe_tree(f.workspace), before)
            self.assertEqual(moves(), 0)

    def test_competing_same_key(self):
        f = self.fixture()
        with watch_target(f.workspace) as moves:
            workers = []
            try:
                for _ in range(2):
                    workers.append(f.start())
                for child in workers:
                    child.stdin.write(canonical(f.proposal))
                    child.stdin.close()
                    child.stdin = None
                replies = [f.finish(child) for child in workers]
                published = [read for code, read in replies if code == 0 and read["status"] == "published"]
                self.assertTrue(published, replies)
                for code, read in replies:
                    if code:
                        self.assertEqual(read["status"], "busy")
                    else:
                        self.assertEqual(read["attempt"], published[0]["attempt"])
                self.assertEqual(moves(), 1)
                self.assertEqual(self.reply(f, "published")["attempt"], published[0]["attempt"])
                journal = json.loads(f.slot.journal.read_bytes())
                self.assertEqual(journal["attempts"], [published[0]["attempt"]])
                self.assertEqual(moves(), 0)
            finally:
                for child in workers:
                    terminate(child)

    def test_stage_cleanup_and_foreign_identity(self):
        f = self.fixture(label="owned")
        baseline = snapshot(f.slot.journal), snapshot(f.slot.anchor), snapshot(f.target)
        _, failure = f.submit(point="intent.write", action="short", fail_cleanup=True)
        self.assertEqual(failure["warning"], "owned_stage_cleanup_failed")
        self.assertEqual(f.slot.health.read_bytes(), b"0")
        self.assertEqual((snapshot(f.slot.journal), snapshot(f.slot.anchor), snapshot(f.target)), baseline)
        self.assertTrue((f.slot.store / ".gaap-stage-append").is_file())
        f.slot.call("lookup", f.key, expected="invalid_store")
        f = self.fixture(label="replaced")
        baseline = snapshot(f.slot.journal), snapshot(f.slot.anchor), snapshot(f.target)
        with f.stopped("intent.before_rename") as worker:
            stage = f.slot.store / ".gaap-stage-append"
            foreign = f.root / "foreign"
            write_private(foreign, b"foreign stage")
            os.replace(foreign, stage)
            kept = snapshot(stage)
            worker.send_signal(signal.SIGCONT)
            code, reply = f.finish(worker)
            self.assertNotEqual(code, 0, reply)
            self.assertEqual(snapshot(stage), kept)
        self.assertEqual((snapshot(f.slot.journal), snapshot(f.slot.anchor), snapshot(f.target)), baseline)
        f.slot.call("lookup", f.key, expected="invalid_store")

    def test_failed_verifier_after_observed_replacement(self):
        content = "def shipping_quote(quantity, unit_price):\n    return 9\n"
        f = self.fixture(content)
        with watch_target(f.workspace) as moves:
            sealed = self.reply(f, "published")["attempt"]
            stored = f.slot.call("lookup", f.key, expected="published")
            self.assertEqual(stored["attempt"], sealed)
            row = facts(stored)
            self.assertEqual(row["observed"]["disposition"], "replaced")
            self.assertEqual(row["verified"]["event"]["verdict"], "fail")
            self.assertNotEqual(row["published"]["receipt"]["body"]["terminal_status"], "completed")
            self.assertEqual(f.target.read_bytes(), content.encode())
            self.assertEqual(moves(), 1)
            self.assertEqual(self.reply(f, "published")["attempt"], sealed)
            self.assertEqual(moves(), 0)

    def test_target_death_and_namespace_hold(self):
        for point, replaced in (("target.before_rename", False), ("target.after_rename", True)):
            with self.subTest(point=point):
                f = self.fixture(label=point)
                with watch_target(f.workspace) as moves:
                    with f.stopped(point) as worker:
                        self.assertTrue(lock_blocked(f.slot.lock))
                        self.assertEqual(f.slot.health.read_bytes(), b"0")
                        live = json.loads(f.slot.journal.read_bytes())
                        self.assertEqual([x["tag"] for x in live["attempts"][0]["facts"]], ["intent", "dispatch"])
                        self.assertEqual(json.loads(f.slot.anchor.read_bytes())["journal_digest"],
                                         digest(f.slot.journal.read_bytes()))
                        self.assertEqual(record_file(f.target)["ino"] != f.initial["ino"], replaced)
                        stages = [p for p in f.workspace.iterdir() if p.name not in self.inputs]
                        self.assertEqual(len(stages), int(not replaced))
                        if stages:
                            self.assertEqual(stages[0].read_bytes(), GOOD.encode())
                        self.assertEqual(moves(), int(replaced))
                        process_handle = os.pidfd_open(worker.pid)
                        try:
                            signal.pidfd_send_signal(process_handle, signal.SIGKILL)
                            self.assertEqual(worker.wait(timeout=8), -signal.SIGKILL)
                            self.assertTrue(select.select([process_handle], [], [], 0)[0])
                            info = Path(f"/proc/self/fdinfo/{process_handle}").read_text().splitlines()
                            self.assertIn("Pid:\t-1", info)
                        finally:
                            os.close(process_handle)
                    self.assertFalse(lock_blocked(f.slot.lock))
                    retained = f.slot.call("lookup", f.key, expected="partial")
                    self.assertTrue(retained["workspace_hold"])
                    self.assertEqual(retained["attempt"], live["attempts"][0])
                    before = observe_tree(f.workspace), snapshot(f.slot.journal), snapshot(f.slot.anchor)
                    self.assertEqual(self.reply(f, "partial")["attempt"], retained["attempt"])
                    self.reply(f, "workspace_held", key="new-key")
                    f.slot.host["principal"] = "bob"
                    f.slot.configure()
                    self.assertTrue(f.slot.call("lookup", f.key, expected="absent")["workspace_hold"])
                    self.reply(f, "workspace_held", key=f.key)
                    self.assertEqual((observe_tree(f.workspace), snapshot(f.slot.journal), snapshot(f.slot.anchor)), before)
                    self.assertEqual(moves(), 0)

    def test_same_dispatch_across_the_visible_target_rename(self):
        f = self.fixture()
        with f.stopped("target.before_rename") as worker, f.slot.journal.open("rb") as dispatch:
            original = dispatch.read()
            self.assertEqual([e["tag"] for e in json.loads(original)["attempts"][0]["facts"]],
                             ["intent", "dispatch"])
            self.assertEqual(record_file(f.target), f.initial)
            self.assertEqual(digest(original), json.loads(f.slot.anchor.read_bytes())["journal_digest"])
            with watch_publications(f) as ordered:
                worker.send_signal(signal.SIGCONT)
                code, reply = f.finish(worker)
                self.assertEqual((code, reply["status"]), (0, "published"))
                # No journal replacement intervened between the confirmed dispatch and the file effect.
                self.assertEqual(ordered(), ["shipping.py", "journal.json", "journal.json", "journal.json"])
            dispatch.seek(0)
            self.assertEqual(dispatch.read(), original)
            self.assertEqual(f.target.read_bytes(), GOOD.encode())

    def test_journal_and_anchor_cuts(self):
        # Read the canonical pair from outside while locks are held. Never repair a one-ahead store.
        for phase in ("intent", "dispatch", "observed", "verified", "published"):
            for stage in (phase, "confirm_" + phase):
                for cut in ("before_rename", "after_rename", "before_dir_sync", "after_dir_sync"):
                    point = stage + "." + cut
                    with self.subTest(point=point):
                        f = self.fixture(label=point)
                        with watch_target(f.workspace) as moves:
                            with f.stopped(point):
                                self.assertTrue(lock_blocked(f.slot.lock))
                                self.assertEqual(f.slot.health.read_bytes(), b"0")
                            journal = json.loads(f.slot.journal.read_bytes())
                            anchor = json.loads(f.slot.anchor.read_bytes())
                            moved = phase in ("observed", "verified", "published")
                            self.assertEqual(moves(), int(moved))
                            self.assertEqual(record_file(f.target)["ino"] != f.initial["ino"], moved)
                            self.assertEqual(f.target.read_bytes(), GOOD.encode() if moved else self.inputs["shipping.py"])
                            pending_stage = stage == phase and cut == "before_rename"
                            if pending_stage:
                                self.assertTrue(any(p.name != "journal.json" for p in f.slot.store.iterdir()))
                                status = "invalid_store"
                            elif journal["generation"] == anchor["generation"] + 1:
                                status = "unconfirmed"
                            else:
                                self.assertEqual(anchor["journal_digest"], digest(f.slot.journal.read_bytes()))
                                self.assertEqual(anchor["generation"], journal["generation"])
                                status = "published" if phase == "published" else "partial"
                            retained = f.slot.call("lookup", f.key, expected=status)
                            before = snapshot(f.slot.anchor), snapshot(f.slot.journal), observe_tree(f.workspace)
                            again = self.reply(f, status)
                            if status in ("published", "partial"):
                                self.assertEqual(again["attempt"], retained["attempt"])
                            else:
                                self.assertNotIn("attempt", retained)
                            self.assertEqual((snapshot(f.slot.anchor), snapshot(f.slot.journal), observe_tree(f.workspace)), before)
                            self.assertEqual(moves(), 0)

    def test_lost_reply_after_confirmation(self):
        f = self.fixture()
        with watch_target(f.workspace) as moves:
            with f.stopped("submit.before_response") as worker:
                self.assertFalse(select.select([worker.stdout], [], [], 0)[0])
                self.assertTrue(lock_blocked(f.slot.lock))
                self.assertEqual(moves(), 1)
            stored = f.slot.call("lookup", f.key, expected="published")
            before = snapshot(f.slot.journal), snapshot(f.slot.anchor), observe_tree(f.archive)
            self.assertEqual(self.reply(f, "published")["attempt"], stored["attempt"])
            self.assertEqual((snapshot(f.slot.journal), snapshot(f.slot.anchor), observe_tree(f.archive)), before)
            self.assertEqual(moves(), 0)

    def test_stale_authority_and_revocation_ordering(self):
        for label, change in (("revoked", {"revoked": True}), ("cancelled", {"cancelled": True}),
                              ("budget", {"budget": {**self.budget, "max_tool_calls": 0}}),
                              ("expiry", None)):
            with self.subTest(label=label):
                f = self.fixture(label=label)
                if change is None:
                    # Set a real deadline before admission; expire the same unchanged revision while stopped.
                    deadline = time.time_ns() // 1_000_000 + 3000
                    update_grant(f.authority, expires_at_ms=deadline)
                with watch_target(f.workspace) as moves, f.stopped("target.before_rename") as worker:
                    self.assertFalse(lock_blocked(f.authority_lock))
                    if change is None:
                        time.sleep(max(0, (deadline + 20) / 1000 - time.time()))
                    else:
                        update_grant(f.authority, **change)
                    worker.send_signal(signal.SIGCONT)
                    code, answer = f.finish(worker)
                    self.assertEqual(record_file(f.target), f.initial)
                    self.assertEqual(moves(), 0)
                    read = f.slot.call("lookup", f.key, expected=("partial", "published"))
                    row = facts(read)
                    self.assertIn("dispatch", row)
                    self.assertNotEqual(row.get("observed", {}).get("disposition"), "replaced")
                    if read["status"] == "published":
                        self.assertNotEqual(row["published"]["receipt"]["body"]["terminal_status"], "completed")
                    self.assertEqual(self.reply(f, read["status"])["attempt"], read["attempt"])
                    self.assertEqual(moves(), 0, (code, answer))
        f = self.fixture(label="guarded-rename")
        with watch_target(f.workspace) as moves, f.stopped("target.authority_held_before_rename") as worker:
            self.assertTrue(lock_blocked(f.authority_lock))
            # Use a fresh process with a separate lock description. Its observation happens under the exclusive lock.
            updater = subprocess.Popen([sys.executable, "-B", str(Path(__file__).resolve()), "--revoke",
                                        f.authority, f.target], stdout=subprocess.PIPE, text=True)
            try:
                self.assertTrue(select.select([updater.stdout], [], [], 5)[0])
                self.assertEqual(updater.stdout.readline().strip(), "waiting")
                worker.send_signal(signal.SIGCONT)
                output, _ = updater.communicate(timeout=10)
                self.assertEqual(updater.returncode, 0)
                self.assertEqual(json.loads(output)["ino"], record_file(f.target)["ino"])
                self.assertNotEqual(record_file(f.target)["ino"], f.initial["ino"])
                f.finish(worker)
                self.assertEqual(moves(), 1)
            finally:
                if updater.poll() is None:
                    updater.kill()
                    updater.communicate()

    def test_revoke_after_effect_before_completion(self):
        f = self.fixture()
        with watch_target(f.workspace) as moves, f.stopped("verified.after_dir_sync") as worker:
            self.assertEqual(moves(), 1)
            self.assertFalse(lock_blocked(f.authority_lock))
            update_grant(f.authority, revoked=True)
            worker.send_signal(signal.SIGCONT)
            f.finish(worker)
            read = f.slot.call("lookup", f.key, expected=("published", "partial"))
            row = facts(read)
            self.assertEqual(row["observed"]["disposition"], "replaced")
            self.assertEqual(row["verified"]["event"]["verdict"], "pass")
            if "published" in row:
                self.assertNotEqual(row["published"]["receipt"]["body"]["terminal_status"], "completed")
            self.assertEqual(f.target.read_bytes(), GOOD.encode())
            self.assertEqual(self.reply(f, read["status"])["attempt"], read["attempt"])
            self.assertEqual(moves(), 0)

    def test_witness_loss_and_archive_faults(self):
        f = self.fixture(label="witness")
        with watch_target(f.workspace) as moves, f.stopped("target.before_rename") as worker:
            pids = Path(f"/proc/{worker.pid}/task/{worker.pid}/children").read_text().split()
            witnesses = []
            for pid in pids:
                child = Path("/proc") / pid
                if b"__journal-witness" in (child / "cmdline").read_bytes().split(b"\0"):
                    witnesses.append(child)
            self.assertEqual(len(witnesses), 1, pids)
            child, = witnesses
            self.assertTrue(os.path.samefile(child / "exe", ARGS.fault_reference))
            handle = os.pidfd_open(int(child.name))
            try:
                inherited = {str(fd.resolve()) for fd in (child / "fd").iterdir()}
                self.assertFalse({str(f.slot.lock), str(f.slot.health), str(f.slot.config)} & inherited)
                signal.pidfd_send_signal(handle, signal.SIGKILL)
                self.assertTrue(select.select([handle], [], [], 6)[0])
                worker.send_signal(signal.SIGCONT)
                f.finish(worker)
                self.assertIn(moves(), (0, 1))
                read = f.slot.call("lookup", f.key, expected=("partial", "published"))
                self.assertTrue(read["workspace_hold"])
                self.reply(f, "workspace_held", key="another-key")
                self.assertEqual(moves(), 0)
            finally:
                os.close(handle)
        # Stages and capsules on either side of publication can't advance the journal on restart.
        for point, has_write in (("archive_grant.before_rename", False), ("archive_effect.before_rename", True),
                                 ("archive_effect.after_rename", True), ("archive_verifier.after_rename", True)):
            with self.subTest(point=point):
                f = self.fixture(label=point)
                with watch_target(f.workspace) as moves:
                    with f.stopped(point):
                        self.assertEqual(moves(), int(has_write))
                    before = snapshot(f.slot.journal), snapshot(f.slot.anchor), observe_tree(f.archive)
                    read = f.slot.call("lookup", f.key, expected=("partial", "invalid_store", "unavailable"))
                    if read["status"] == "partial":
                        row = facts(read)
                        self.assertNotIn("published", row)
                        if point.startswith("archive_effect"):
                            self.assertNotIn("observed", row)
                            self.assertTrue(read["workspace_hold"])
                    again = self.reply(f, read["status"])
                    self.assertEqual(again.get("attempt"), read.get("attempt"))
                    self.assertEqual((snapshot(f.slot.journal), snapshot(f.slot.anchor), observe_tree(f.archive)), before)
                    self.assertEqual(moves(), 0)

    def test_real_target_and_archive_sync_errors(self):
        tracer = shutil.which("strace")
        self.assertIsNotNone(tracer)
        f = self.fixture(label="calibrate")
        healthy = ARGS.evidence / "shipping-healthy.strace"
        trace = [tracer, "-D", "-qq", "-yy", "-e", "trace=fsync,renameat,renameat2"]
        self.reply(f, "published", trace=[*trace, "-o", healthy])
        fsyncs = 0
        effect_capsule = False
        ordinals = {}
        for line in healthy.read_text().splitlines():
            if "renameat" in line and ".effect." in line and "= 0" in line:
                effect_capsule = True
            if "fsync(" in line:
                fsyncs += 1
                if f"<{f.workspace}>" in line:
                    ordinals["target"] = fsyncs
                if effect_capsule and f"<{f.archive}>" in line:
                    ordinals["archive_effect"] = fsyncs
                    effect_capsule = False
        self.assertEqual(set(ordinals), {"target", "archive_effect"})
        for role, ordinal in ordinals.items():
            with self.subTest(role=role):
                f = self.fixture(label=role)
                output = ARGS.evidence / f"shipping-{role}-eio.strace"
                injected = [*trace, "-o", output, "-e", f"inject=fsync:error=EIO:when={ordinal}"]
                with watch_target(f.workspace) as moves:
                    code, reply = f.submit(trace=injected)
                    self.assertNotEqual(code, 0, reply)
                    directory = f.workspace if role == "target" else f.archive
                    self.assertTrue(any(f"<{directory}>" in line and "EIO" in line and "INJECTED" in line
                                        for line in output.read_text().splitlines()))
                    self.assertEqual(moves(), 1)
                    self.assertEqual(f.target.read_bytes(), GOOD.encode())
                    self.assertEqual(f.slot.health.read_bytes(), b"1")
                    self.assertNotIn("attempt", f.slot.call("lookup", f.key, expected="unavailable"))
                    self.reply(f, "unavailable", key="other")
                    self.assertEqual(moves(), 0)
        f = self.fixture(label="pending")
        output = ARGS.evidence / "shipping-unacknowledged-eio.strace"
        injected = [*trace, "-o", output, "-e", f"inject=fsync:error=EIO:when={ordinals['target']}"]
        with watch_target(f.workspace) as moves:
            with f.stopped("volume.after_eio", trace=injected):
                self.assertEqual(f.slot.health.read_bytes(), b"P")
            self.assertTrue(any(f"<{f.workspace}>" in line and "EIO" in line and "INJECTED" in line
                                for line in output.read_text().splitlines()))
            self.assertNotIn("attempt", f.slot.call("lookup", f.key, expected="unavailable"))
            self.reply(f, "unavailable")
            self.assertEqual(f.slot.health.read_bytes(), b"P")
            self.assertEqual(moves(), 1)

    def tearDown(self):
        reap_debugger_children(Path("/proc/thread-self/children"))


def main():
    """Run independently of earlier suites, retaining per-fault raw evidence in the requested directory."""
    global ARGS
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("release", "qualification", "reference", "fault-reference", "root", "health-root", "evidence"):
        parser.add_argument("--" + name, required=True, type=lambda value: Path(value).resolve(strict=True))
    ARGS = parser.parse_args()
    if sys.platform != "linux" or os.getuid() == 0:
        parser.error("run as the ordinary service user on private Linux ext4")
    if ARGS.root.stat().st_dev == ARGS.health_root.stat().st_dev:
        parser.error("supply an independent device for the health file")
    os.umask(0o077)
    prctl = ctypes.CDLL(None, use_errno=True).prctl
    prctl.argtypes = [ctypes.c_int] + [ctypes.c_ulong] * 4
    if prctl(36, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "couldn't adopt interrupted witness processes")
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(JournalSubmit)
    return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--revoke":
        authority, target = map(Path, sys.argv[2:])
        if not lock_blocked(authority / "authority.lock"):
            raise AssertionError("revocation didn't encounter the worker's authority fence")
        print("waiting", flush=True)
        update_grant(authority, revoked=True)
        print(json.dumps(record_file(target)))
    else:
        sys.exit(main())

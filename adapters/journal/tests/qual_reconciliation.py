"""Qualify v0.1 coexistence, explicit cutover and operator rulings on a disposable ext4 slot."""

import argparse
import copy
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time
import unittest

from qual_executor_quiescence import watch_target
from qual_history import GOOD, exercise_unknown, number_canonical, reference, reference_profile
from qual_host import Slot, canonical, digest, identity, observe_tree, snapshot, write_private
from qual_operator_host import Operator
from qual_operator_observer import observe
from qual_operator_seal import check_sealed_failure
from qual_submit_host import Shipping, subject, terminate, update_grant


def read(path):
    """Decode the JSON file from disk so the test can check the CLI's reply against it."""
    return json.loads(path.read_bytes())


def pair(slot):
    """Capture the identity and content digest of the slot's anchor and journal."""
    return snapshot(slot.anchor), snapshot(slot.journal)


def save(case, slot, operator, log):
    """Copy the final slot, authority and observer evidence before fixture cleanup removes the workspace."""
    case.mkdir(parents=True)
    for name, src in (("anchor.json", slot.anchor), ("journal.json", slot.journal), ("health", slot.health)):
        if src.exists():
            shutil.copyfile(src, case / name)
    for name, src in (("candidates", operator.candidates), ("authority", operator.authority)):
        shutil.copytree(src, case / name)
    (case / "observed.json").write_bytes(canonical({
        "workspace": observe_tree(slot.workspace), "control": observe_tree(slot.control),
        "store": observe_tree(slot.store), "commands": log, "operator_commands": operator.commands}))


def kill_stopped(child):
    """Kill the child at its qualification stop, require SIGKILL on reap and drain its pipes."""
    child.kill()
    assert child.wait(timeout=5) == -signal.SIGKILL
    child.communicate(timeout=3)


def send_proposal(child, proposal):
    """Deliver one request to a contender and close its input before waiting for its lock."""
    child.stdin.write(canonical(proposal))
    child.stdin.close()
    child.stdin = None


def contender(child, executable, pin, *, minimum=1, owned=False):
    """Read the actual executable, open lock descriptors and kernel flock records while the owner is live."""
    proc = Path(f"/proc/{child.pid}")
    limit = time.monotonic() + 2
    expected = pin.stat()
    while time.monotonic() < limit and child.poll() is None:
        try:
            if os.path.samefile(proc / "exe", executable):
                descriptors = {}
                for fd in (proc / "fd").iterdir():
                    info = fd.stat()
                    if (info.st_dev, info.st_ino) == (expected.st_dev, expected.st_ino):
                        raw = (proc / "fdinfo" / fd.name).read_text()
                        descriptors[fd.name] = [line for line in raw.splitlines() if line.startswith("lock:")]
                locked = any(" FLOCK " in line for lines in descriptors.values() for line in lines)
                if len(descriptors) >= minimum and (locked or not owned):
                    return {"pid": child.pid, "executable": str(executable), "pin": identity(pin),
                            "descriptors": descriptors}
        except FileNotFoundError:
            pass
        time.sleep(.001)
    raise AssertionError(("contender missed pinned lock", child.pid, str(executable), str(pin), child.poll()))


class Reconciliation(unittest.TestCase):
    """Exercise old and new journal clients against the same disposable ext4 slot."""

    @classmethod
    def setUpClass(cls):
        """Run the shipping reference once to obtain original input bytes, the historical profile and its budget."""
        with tempfile.TemporaryDirectory(dir=ARGS.root) as tmp:
            legacy = Path(tmp) / "legacy"
            initial = reference(ARGS.reference, legacy, "coding-init")
            cls.inputs = {p.name: p.read_bytes() for p in (legacy / "workspace").iterdir()}
            run = reference(ARGS.reference, legacy, "coding-submit", {
                "path": "shipping.py", "content": GOOD, "plan": "Implement shipping rules.",
                "base_digest": initial["subject_digest"]})
            cls.profile, cls.budget = reference_profile(run), run["request"]["resource_budget"]

    def area(self, root):
        """Create a case-owned directory under the selected volume and remove it after evidence collection."""
        tmp = tempfile.TemporaryDirectory(dir=root)
        self.addCleanup(tmp.cleanup)
        return Path(tmp.name)

    def fixture(self, label="", content=GOOD):
        """Enroll a shipping slot and register its evidence copy to run before the temporary directories disappear."""
        root, health = self.area(ARGS.root), self.area(ARGS.health_root)
        shipping = Shipping(root, health, ARGS, self.inputs, self.profile, self.budget, content, "shipping-1")
        operator = Operator(shipping.slot, root, health)
        dest = ARGS.evidence / "operator" / (self._testMethodName + ("-" + label if label else ""))
        self.addCleanup(save, dest, shipping.slot, operator, shipping.commands)
        return shipping, operator

    def converted(self, shipping, operator):
        """Check that conversion preserves the source generation and attempts and binds both file digests
        to the new anchor.
        """
        old_anchor, old_journal = shipping.slot.anchor.read_bytes(), shipping.slot.journal.read_bytes()
        old = json.loads(old_journal)
        result = operator.convert()
        new, anchor = read(shipping.slot.journal), read(shipping.slot.anchor)
        self.assertEqual(result["generation"], old["generation"])
        self.assertEqual(new["generation"], old["generation"])
        self.assertEqual(number_canonical(old["attempts"]), number_canonical(new["attempts"]))
        self.assertEqual(new["rulings"], [])
        self.assertEqual(anchor["journal_digest"], digest(shipping.slot.journal.read_bytes()))
        self.assertEqual(anchor["pending"], None)
        self.assertEqual(anchor["origin"], dict(source_generation=old["generation"],
            source_attempt_count=len(old["attempts"]), source_anchor_digest=digest(old_anchor),
            source_journal_digest=digest(old_journal), initial_journal_digest=anchor["journal_digest"]))
        self.assertEqual(new["reconciliation_profile_digest"], operator.profile_digest)
        self.assertEqual(anchor["format"], "gaap.journal-anchor/0.2")
        self.assertEqual(new["format"], "gaap.journal/0.2")
        shipping.commands.append({"convert": result, "origin": anchor["origin"]})
        return old

    def test_conversion_preserves_originals(self):
        """Compare empty, partial and sealed v0.1 histories across conversion while watching the target for writes."""
        for kind in ("empty", "refused", "failed", "completed", "partial"):
            with self.subTest(kind=kind):
                bad = "def shipping_quote(quantity, unit_price):\n    return 7\n"
                shipping, operator = self.fixture(kind, bad if kind == "failed" else GOOD)
                if kind == "refused":
                    shipping.proposal.update(path="deployment.json", content="{}")
                    update_grant(shipping.authority, proposal_digest=digest(canonical(shipping.proposal)))
                if kind == "partial":
                    with operator.old_handoff(), shipping.stopped("target.before_rename") as child:
                        kill_stopped(child)
                elif kind != "empty":
                    with operator.old_handoff():
                        code, sealed = shipping.submit(binary=ARGS.old_reference)
                        self.assertEqual((code, sealed["status"]), (0, "published"))
                        receipt = sealed["attempt"]["facts"][-1]["payload"]["receipt"]
                        self.assertEqual(receipt["body"]["terminal_status"] == "completed", kind == "completed")
                operator.pin_source()
                status = "absent" if kind == "empty" else "partial" if kind == "partial" else "published"
                legacy = shipping.lookup(shipping.key, expected=status)
                with watch_target(shipping.workspace) as moves:
                    self.converted(shipping, operator)
                    after = shipping.lookup(shipping.key, expected=status)
                    self.assertEqual(legacy.get("attempt"), after.get("attempt"))
                    self.assertEqual(after["workspace_hold"], legacy["workspace_hold"])
                    self.assertEqual(moves(), 0)

    def test_original_identity_reuse(self):
        """Duplicate an admission in a rehashed journal and require lookup to withhold even an unrelated key."""
        shipping, operator = self.fixture()
        self.converted(shipping, operator)
        code, result = shipping.submit()
        self.assertEqual((code, result["status"]), (0, "published"))
        journal = read(shipping.slot.journal)
        # Duplicate a whole frozen-valid envelope after its run completed, preserving
        # the untouched generation-zero origin. Rehash and confirm the otherwise valid pair.
        admission = journal["attempts"][0]
        journal["attempts"].append(copy.deepcopy(admission))
        journal["generation"] += len(admission["facts"])
        content = number_canonical(journal)
        anchor = read(shipping.slot.anchor)
        anchor.update(generation=journal["generation"], journal_digest=digest(content))
        write_private(shipping.slot.journal, content)
        write_private(shipping.slot.anchor, canonical(anchor))
        before = pair(shipping.slot), snapshot(shipping.target)
        denied = operator.command("lookup", "unrelated", expected="invalid_store")
        self.assertNotIn("attempt", denied)
        self.assertEqual(before, (pair(shipping.slot), snapshot(shipping.target)))

    def test_legacy_sealed_unknown(self):
        """Check that conversion preserves an unknown effect's sealed receipt and keeps the workspace held."""
        self.root, self.health = self.area(ARGS.root), self.area(ARGS.health_root)
        self.slot = lambda **kwargs: Slot(self.root, self.health, ARGS.old_release, ARGS.qualification, **kwargs)
        self.record = lambda slot, name, result: setattr(self, "held_slot", slot)
        exercise_unknown(self, ARGS.old_reference, ARGS.evidence)
        slot = self.held_slot
        slot.release = ARGS.release
        prior = slot.call("lookup", "uncertain", expected="published")
        operator = Operator(slot, self.root, self.health)
        history = prior["attempt"]
        result = operator.convert()
        after = slot.call("lookup", "uncertain", expected="published")
        self.assertEqual(after["attempt"], history)
        self.assertTrue(after["workspace_hold"])
        self.assertEqual(history["facts"][-1]["payload"]["result"]["body"]["execution_status"], "unknown_outcome")
        save(ARGS.evidence / "operator" / self._testMethodName, slot, operator,
             [{"convert": result, "unknown_receipt": history["facts"][-1]["payload"]["receipt"]["receipt_digest"]}])

    def test_mixed_writers(self):
        """Race both generations under the original locks and count every target replacement."""
        shipping, operator = self.fixture("old-wins")
        with watch_target(shipping.workspace) as moves:
            # Hold the original issuer so the released worker pauses inside its live slot transaction.
            with shipping.authority_lock.open("rb") as grant_lock:
                fcntl.flock(grant_lock, fcntl.LOCK_EX)
                with operator.writers.open("rb") as writer_lease, operator.old_handoff():
                    fcntl.flock(writer_lease, fcntl.LOCK_SH)
                    writer = shipping.start(binary=ARGS.old_reference, supervisor_fds=((8, writer_lease.fileno()),))
                    self.addCleanup(terminate, writer)
                    send_proposal(writer, shipping.proposal)
                owners = {
                    "writer_lease": contender(writer, ARGS.old_reference, operator.writers, owned=True),
                    "slot_owner": contender(writer, ARGS.old_reference, shipping.slot.lock, owned=True),
                    "old_waiting_on_issuer": contender(writer, ARGS.old_reference, shipping.authority_lock)}
                converter = shipping.slot.start(["convert"])
                self.addCleanup(terminate, converter)
                owners["converter_waiting_on_lease"] = contender(converter, ARGS.release, operator.writers)
                self.assertEqual(read(shipping.slot.anchor)["generation"], 0)
            code, old = shipping.finish(writer)
            self.assertEqual((code, old["status"]), (0, "published"))
            code, stale = shipping.finish(converter)
            self.assertEqual((code, stale["status"], stale["reason"]), (1, "conflict", "conversion_source_changed"))
            self.assertEqual((read(shipping.slot.anchor)["generation"], moves()), (5, 1))
            self.assertEqual(read(shipping.slot.anchor)["format"], "gaap.journal-anchor/0.1")
            shipping.commands.append({"contention": owners, "stale_origin": stale, "target_moves": 1})

        shipping, operator = self.fixture("cutover-wins")
        with watch_target(shipping.workspace) as moves:
            converter = shipping.slot.stop("cutover_fence.after_dir_sync", command=("convert",))
            self.addCleanup(terminate, converter)
            owners = {"converter_slot": contender(converter, ARGS.qualification, shipping.slot.lock, owned=True),
                      "converter_health": contender(converter, ARGS.qualification, shipping.slot.health, owned=True),
                      "converter_lease": contender(converter, ARGS.qualification, operator.writers, owned=True)}
            with operator.old_handoff():
                old_writer = shipping.start(binary=ARGS.old_reference)
                old_enroll = shipping.slot.start(["enroll"], binary=ARGS.old_release)
            new_writer = shipping.start()
            for child in (old_writer, old_enroll, new_writer):
                self.addCleanup(terminate, child)
            for child in (old_writer, new_writer):
                send_proposal(child, shipping.proposal)
            # fd 4 is the inherited health file. Its separate duplicate and flock description
            # show both released clients reached lock_health while the converter still owned it.
            owners["old_writer_waiting_on_health"] = contender(old_writer, ARGS.old_reference, shipping.slot.health, minimum=3)
            owners["old_enroll_waiting_on_health"] = contender(old_enroll, ARGS.old_release, shipping.slot.health, minimum=3)
            owners["new_writer_waiting_on_lease"] = contender(new_writer, ARGS.reference, operator.writers)
            kill_stopped(converter)
            for child, status in ((old_writer, "unsupported_storage"), (old_enroll, "unsupported_storage"),
                                  (new_writer, "cutover_pending")):
                code, reply = shipping.finish(child)
                self.assertEqual((code, reply["status"]), (1, status))
            operator.command("lookup", shipping.key, expected="cutover_pending")
            self.assertEqual(moves(), 0)
            self.assertEqual(operator.convert()["generation"], 0)
            with operator.old_handoff():
                operator.command("lookup", shipping.key, binary=ARGS.old_release, expected="unsupported_storage")
            self.assertEqual(moves(), 0)
            shipping.commands.append({"contention": owners, "target_moves": 0})

    def test_conversion_interruptions(self):
        """Kill the converter around each rename and sync, then check the permitted retry against the
        surviving files.
        """
        for stage in ("cutover_fence", "cutover_journal", "cutover_active"):
            for cut in ("before_rename", "after_rename", "before_dir_sync", "after_dir_sync"):
                with self.subTest(stage=stage, cut=cut):
                    point = stage + "." + cut
                    shipping, operator = self.fixture(point)
                    initial = snapshot(shipping.target)
                    child = shipping.slot.stop(point, command=("convert",))
                    kill_stopped(child)
                    anchor, journal = read(shipping.slot.anchor), read(shipping.slot.journal)
                    self.assertEqual(shipping.slot.health.read_text(), "0")
                    if cut == "before_rename":
                        directory = shipping.slot.store if stage == "cutover_journal" else shipping.slot.control
                        leftovers = [x for x in directory.iterdir() if x.name.startswith(".")]
                        self.assertTrue(leftovers)
                        before = observe_tree(directory)
                        operator.command("convert", expected="invalid_store")
                        self.assertEqual(observe_tree(directory), before)
                    else:
                        self.assertEqual(anchor["phase"], "active" if stage == "cutover_active" else "cutover")
                        self.assertEqual(journal["format"], "gaap.journal/0.1" if stage == "cutover_fence" else "gaap.journal/0.2")
                        self.assertEqual(operator.convert()["generation"], 0)
                    self.assertEqual(snapshot(shipping.target), initial)
                    shipping.commands.append({"cut": point, "observed_anchor": anchor, "observed_format": journal["format"]})
        shipping, operator = self.fixture("lost-response")
        kill_stopped(shipping.slot.stop("convert.before_response", command=("convert",)))
        before = pair(shipping.slot)
        self.assertEqual(operator.convert()["generation"], 0)
        self.assertEqual(before, pair(shipping.slot))

    def interrupted(self, kind, label):
        """Stop a real shipping worker and retain the observer's deposit alongside its dispatch-only history."""
        shipping, operator = self.fixture(label)
        self.converted(shipping, operator)
        entry = observe(shipping, operator, kind)
        source = operator.deposit(entry)
        original = shipping.lookup(shipping.key, expected="partial")["attempt"]
        self.assertEqual([fact["tag"] for fact in original["facts"]], ["intent", "dispatch"])
        shipping.commands.append({"observed_disposition": kind, "attempt_id": entry["attempt_id"],
                           "source": source, "pidfd": entry["stop"], "target": snapshot(shipping.target)})
        return shipping, operator, entry, source, original

    def test_unchanged_sealed_failure_after_ruling(self):
        """Read the original failed receipt after a ruling and a duplicate submission, with the workspace
        under watch.
        """
        check_sealed_failure(self)

    def test_all_observed_outcomes_and_original_seals(self):
        """Admit each supported observation and verify that its interrupted run stays unsealed on lookup and retry."""
        for kind in ("not_started", "replaced", "stopped_before_rename"):
            with self.subTest(kind=kind):
                shipping, operator, record, source, original = self.interrupted(kind, kind)
                with watch_target(shipping.workspace) as moves:
                    ruled = operator.reconcile(record["attempt_id"], source)
                    self.assertEqual(ruled["ruling"]["disposition"], kind)
                    self.assertEqual(ruled["ruling"]["admitted_by"], "alice")
                    self.assertFalse(ruled["workspace_hold"])
                    after = shipping.lookup(shipping.key, expected="partial")
                    self.assertEqual(after["rulings"], [ruled["ruling"]])
                    self.assertEqual(after["attempt"], original)
                    self.assertEqual((shipping.submit()[1]["attempt"], moves()), (original, 0))

    def test_current_authority_and_incomplete_sources(self):
        """Check source eligibility, private readback and revocation between intake and the operator's
        final decision.
        """
        shipping, operator, record, source, original = self.interrupted("replaced", "authority")
        attempt = record["attempt_id"]
        for right in ("read", "reconcile"):
            operator.rights["alice"][right] = False
            operator.publish_rights()
            state = pair(shipping.slot)
            known = operator.reconcile(attempt, source, expected="forbidden")
            foreign = operator.reconcile("0"*32, digest(b"unknown"), expected="forbidden")
            self.assertEqual(known, foreign)
            self.assertEqual(state, pair(shipping.slot))
            operator.rights["alice"][right] = True
        operator.publish_rights()
        for change in ("wrong_workspace", "wrong_principal", "detached_source", "gap", "foreign_actor"):
            bad = copy.deepcopy(record)
            if change == "wrong_workspace":
                bad["workspace_id"] = "a"*32
            elif change == "wrong_principal":
                bad["gate"]["sender_uid"] += 1
            elif change == "detached_source":
                bad["before"]["files"]["TASK.md"]["content"] += "x"
            elif change == "gap":
                bad["watch"]["armed_ns"] = bad["watch"]["drained_ns"]
            else:
                bad["observer_id"] = "human-operator"
            refused = operator.reconcile(attempt, operator.deposit(bad), expected="ineligible_evidence")
            self.assertNotIn("ruling", refused)
            self.assertTrue(shipping.lookup(shipping.key, expected="partial")["workspace_hold"])
        cmd = ["reconcile", attempt, source]
        child = shipping.slot.stop("reconcile.before_authority", command=cmd)
        self.addCleanup(terminate, child)
        operator.rights["alice"]["reconcile"] = False
        operator.publish_rights()
        child.send_signal(signal.SIGCONT)
        code, denied = shipping.finish(child)
        self.assertTrue(code and denied["status"] == "forbidden", denied)
        self.assertEqual(read(shipping.slot.anchor)["pending"], dict(attempt_id=attempt, evidence_digest=source, intake_by="alice"))
        shipping.slot.host["principal"] = "operator2"
        shipping.slot.configure()
        saved = operator.reconcile(attempt, source)
        self.assertEqual(saved["ruling"]["admitted_by"], "operator2")
        shipping.slot.host["principal"] = "bob"
        shipping.slot.configure()
        absent = shipping.lookup(shipping.key, expected="absent")
        self.assertFalse({"attempt", "rulings", "effect_status", "intake_pending"} & absent.keys())
        operator.reconcile(attempt, source, expected="forbidden")

    def test_pending_crash_matrix_and_duplicate_operators(self):
        """Interrupt each ruling publication step and check what a second operator can recover from the files."""
        for stage in ("intake", "ruling", "confirm_ruling", "clear_intake"):
            for cut in ("before_rename", "after_rename", "before_dir_sync", "after_dir_sync"):
                with self.subTest(stage=stage, cut=cut):
                    point = stage + "." + cut
                    shipping, operator, record, source, original = self.interrupted("not_started", point)
                    cmd = ["reconcile", record["attempt_id"], source]
                    kill_stopped(shipping.slot.stop(point, command=cmd))
                    anchor, journal = read(shipping.slot.anchor), read(shipping.slot.journal)
                    self.assertEqual(shipping.slot.health.read_text(), "0")
                    self.assertEqual(journal["attempts"], [original])
                    shipping.commands.append({"cut": point, "anchor": anchor, "journal_generation": journal["generation"]})
                    if cut == "before_rename":
                        state = pair(shipping.slot)
                        operator.command(*cmd, expected="invalid_store")
                        self.assertEqual(pair(shipping.slot), state)
                    elif stage == "ruling":
                        self.assertEqual(journal["generation"], anchor["generation"] + 1)
                        self.assertEqual(anchor["pending"]["evidence_digest"], source)
                        operator.command(*cmd, expected="unconfirmed")
                        operator.command("lookup", shipping.key, expected="unconfirmed")
                    else:
                        if stage != "clear_intake":
                            self.assertEqual(anchor["pending"]["evidence_digest"], source)
                        saved = operator.reconcile(*cmd[1:])
                        stable = pair(shipping.slot)
                        self.assertEqual(saved, operator.reconcile(*cmd[1:]))
                        self.assertEqual(pair(shipping.slot), stable)
                        self.assertEqual(len(read(shipping.slot.journal)["rulings"]), 1)
                        self.assertIsNone(read(shipping.slot.anchor)["pending"])
        shipping, operator, entry, source, _ = self.interrupted("not_started", "lost-response")
        cmd = ["reconcile", entry["attempt_id"], source]
        child = shipping.slot.stop("reconcile.before_response", command=cmd)
        self.addCleanup(terminate, child)
        competitor = shipping.slot.start(cmd)
        self.addCleanup(terminate, competitor)
        kill_stopped(child)
        code, result = shipping.finish(competitor)
        self.assertEqual((code, result["status"]), (0, "recorded"))
        self.assertEqual(len(read(shipping.slot.journal)["rulings"]), 1)
        self.assertEqual(result, operator.reconcile(*cmd[1:]))

    def fresh(self, shipping, key):
        """Bind a new submission key and grant to the current workspace subject and the revised proposal."""
        shipping.key = key
        shipping.proposal.update(base_digest=subject(shipping.workspace), content=GOOD + "# separately authorized\n")
        approval = dict(shipping.grant["approval"], subject_digest=shipping.proposal["base_digest"], approval_id=key + "-grant")
        update_grant(shipping.authority, principal="alice", key=key, revoked=False,
                     proposal_digest=digest(canonical(shipping.proposal)), approval=approval)

    def equivocation(self, record):
        """Build an intentionally conflicting observer deposit for a dispute test. The original run
        produced only one outcome.
        """
        # The honest run observed a replacement. This deliberately contradictory observer
        # deposit tests dispute handling. It isn't a second physical outcome from that run.
        other = copy.deepcopy(record)
        other["after"] = other["before"]
        other["stage"] = None
        other["gate"]["reply"] = None
        other["watch"].update(events=[], bound_at=0)
        return other

    def test_late_dispute_preserves_intervening_run(self):
        """Admit a contradiction after a later run completes and verify the hold alongside that run's
        original receipt.
        """
        shipping, operator, record, source, original = self.interrupted("replaced", "equivocation")
        attempt = record["attempt_id"]
        original_key = shipping.key
        with watch_target(shipping.workspace) as moves:
            first = operator.reconcile(attempt, source)
            self.assertFalse(first["workspace_hold"])
            self.assertEqual(moves(), 0)
            self.fresh(shipping, "new-job")
            code, sealed = shipping.submit()
            self.assertEqual((code, sealed["status"], moves()), (0, "published", 1))
            receipt = sealed["attempt"]["facts"][-1]["payload"]["receipt"]
            self.assertEqual(receipt["body"]["terminal_status"], "completed")
            contradictory = operator.deposit(self.equivocation(record))
            shipping.commands.append({"simulated_observer_equivocation": contradictory, "genuine_source": source})
            later = operator.reconcile(attempt, contradictory)
            self.assertEqual(later["effect_status"], "disputed")
            self.assertTrue(later["workspace_hold"])
            repeat = operator.reconcile(attempt, source)
            self.assertEqual(repeat["ruling"], first["ruling"])
            self.assertEqual(repeat["generation"], later["generation"])
            self.assertEqual(repeat["effect_status"], "disputed")
            self.assertEqual(shipping.lookup(original_key, expected="partial")["attempt"], original)
            self.assertEqual(shipping.lookup(shipping.key, expected="published")["attempt"], sealed["attempt"])
            self.assertEqual(shipping.submit()[1]["attempt"], sealed["attempt"])
            self.assertEqual(shipping.submit(key="unseen")[1]["status"], "workspace_held")
            self.assertEqual(shipping.submit(stdin=canonical({**shipping.proposal, "plan":"changed"}))[1]["status"], "conflict")
            self.assertEqual(moves(), 0)
        journal = read(shipping.slot.journal)
        self.assertEqual([r["original_fact_count"] for r in journal["rulings"]], [2, 7])
        self.assertEqual([r["recorded_generation"] for r in journal["rulings"]], [3, 9])
        # An authenticated file can still tell an impossible story. Move the second ruling
        # inside the later attempt and rehash the pair so the timeline itself must reject it.
        journal["rulings"][-1]["original_fact_count"] -= 1
        content = number_canonical(journal)
        anchor = read(shipping.slot.anchor)
        anchor["journal_digest"] = digest(content)
        write_private(shipping.slot.journal, content)
        write_private(shipping.slot.anchor, canonical(anchor))
        hidden = operator.command("lookup", "unrelated", expected="invalid_store")
        self.assertNotIn("attempt", hidden)

    def test_quiescence_archive_loss_and_pinned_malformed_input(self):
        """Check that a surviving writer lease, missing evidence or an incomplete entry-gate record
        prevents unsafe progress.
        """
        for label in ("retained-writer", "lost-source", "malformed"):
            with self.subTest(label=label):
                shipping, operator, rec, source, _ = self.interrupted("not_started", label)
                attempt = rec["attempt_id"]
                if label == "retained-writer":
                    with operator.writers.open("rb") as writer_lease:
                        fcntl.flock(writer_lease, fcntl.LOCK_SH)
                        lease_holder = subprocess.Popen(["cat"], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                                                  pass_fds=(writer_lease.fileno(),))
                    try:
                        state = pair(shipping.slot)
                        operator.reconcile(attempt, source, expected="busy")
                        self.assertEqual(pair(shipping.slot), state)
                    finally:
                        lease_holder.communicate(timeout=4)
                    operator.reconcile(attempt, source)
                elif label == "lost-source":
                    operator.reconcile(attempt, source)
                    path = operator.candidates / (source[7:] + ".json")
                    raw = path.read_bytes()
                    path.unlink()
                    self.assertNotIn("attempt", operator.command("lookup", "unknown", expected={"invalid_store", "unavailable"}))
                    self.assertNotEqual(shipping.submit(key="fresh")[0], 0)
                    write_private(path, raw)
                    self.assertFalse(shipping.lookup(shipping.key, expected="partial")["workspace_hold"])
                else:
                    malformed = copy.deepcopy(rec)
                    del malformed["gate"]["reply"]
                    bad = operator.deposit(malformed)
                    operator.reconcile(attempt, bad, expected="invalid_store")
                    self.assertEqual(read(shipping.slot.anchor)["pending"]["evidence_digest"], bad)
                    self.assertTrue(shipping.lookup(shipping.key, expected="partial")["workspace_hold"])
                    operator.reconcile(attempt, source, expected="busy")
                    self.assertEqual(shipping.submit(key="fresh")[1]["status"], "workspace_held")

    def test_capacity_and_storage_withdrawal(self):
        """Exhaust the generation counter and verify that the pending contradiction survives volume
        withdrawal and reopening.
        """
        shipping, operator = self.fixture("exhausted-generation")
        with operator.old_handoff():
            code, original = shipping.submit(binary=ARGS.old_reference)
            self.assertEqual(code, 0)
            legacy = read(shipping.slot.journal)
            legacy["generation"] = 2**53 - 6
            original_bytes = number_canonical(legacy)
            anchor = read(shipping.slot.anchor)
            anchor.update(generation=legacy["generation"], journal_digest=digest(original_bytes))
            write_private(shipping.slot.journal, original_bytes)
            write_private(shipping.slot.anchor, canonical(anchor))
            self.assertEqual(shipping.lookup(shipping.key, expected="published")["attempt"], original["attempt"])
        operator.pin_source()
        self.converted(shipping, operator)
        self.fresh(shipping, "last-key")
        corroborations = []
        rec = observe(shipping, operator, "replaced", corroborations=corroborations)
        attempt = rec["attempt_id"]
        self.assertEqual(len(corroborations), 3)
        for sample in corroborations:
            operator.reconcile(attempt, operator.deposit(sample))
        self.assertEqual(read(shipping.slot.journal)["generation"], 2**53 - 1)
        disagreement = operator.deposit(self.equivocation(rec))
        before = snapshot(shipping.slot.journal)
        operator.reconcile(attempt, disagreement, expected="capacity")
        self.assertEqual(read(shipping.slot.anchor)["pending"]["evidence_digest"], disagreement)
        self.assertEqual(before, snapshot(shipping.slot.journal))
        self.assertTrue(shipping.lookup(shipping.key, expected="partial")["intake_pending"])
        for health in (b"P", b"1", b"0"):
            write_private(shipping.slot.health, health)
            if health != b"0":
                operator.command("lookup", shipping.key, expected="unavailable")
            else:
                operator.reconcile(attempt, disagreement, expected="capacity")
            self.assertEqual(read(shipping.slot.anchor)["pending"]["evidence_digest"], disagreement)

    def test_control_and_archive_failures(self):
        """Check the surviving target and operator response after writeback failure, a foreign stage, an
        unconfirmed journal or lock replacement.
        """
        for label in ("directory-sync", "stage-close", "foreign-stage", "one-ahead", "changed-lock"):
            with self.subTest(label=label):
                shipping, operator = self.fixture(label)
                before = (snapshot(shipping.target), snapshot(shipping.slot.journal))
                if label in ("directory-sync", "stage-close"):
                    if label == "directory-sync":
                        calibration_shipping, calibration_operator = self.fixture("calibrate")
                        trace = ARGS.evidence / "operator-sync-calibration.log"
                        calibration_operator.command("convert", expected="converted", trace=trace)
                        syncs = [line for line in trace.read_text().splitlines() if line.startswith("fsync(")]
                        ordinal = next(i for i, line in enumerate(syncs, 1) if f"<{calibration_shipping.slot.control}>" in line)
                        failed = ARGS.evidence / "operator-sync-eio.log"
                        operator.command("convert", expected="unavailable", trace=failed, fail_sync_ordinal=ordinal)
                        self.assertTrue(any(f"<{shipping.slot.control}>" in line and "EIO" in line and "INJECTED" in line
                                            for line in failed.read_text().splitlines()))
                    else:
                        operator.command("convert", expected="unavailable", point="cutover_fence.close", action="eio")
                    self.assertEqual(shipping.slot.health.read_text(), "1")
                    operator.command("lookup", shipping.key, expected="unavailable")
                elif label == "foreign-stage":
                    stage = shipping.slot.control / ".foreign"
                    write_private(stage, b"another owner")
                    operator.command("convert", expected="invalid_store")
                    self.assertEqual(stage.read_bytes(), b"another owner")
                elif label == "one-ahead":
                    with operator.old_handoff(), shipping.stopped("intent.after_rename") as worker:
                        kill_stopped(worker)
                    operator.command("convert", expected="unconfirmed")
                else:
                    shipping.slot.lock.rename(shipping.slot.control / "former-lock")
                    write_private(shipping.slot.lock, b"")
                    operator.command("convert", expected={"invalid_store", "unavailable"})
                self.assertEqual(snapshot(shipping.target), before[0])


if __name__ == "__main__":
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("release", "qualification", "reference", "fault-reference", "old-release",
                   "old-reference", "root", "health-root", "evidence"):
        parser.add_argument("--" + option, required=True, type=Path)
    ARGS, extra = parser.parse_known_args()
    unittest.main(argv=[__file__, *extra], verbosity=2)

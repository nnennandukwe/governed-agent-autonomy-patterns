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
from qual_host import Slot, canonical, digest, observe_tree, snapshot, write_private
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


def save(case, slot, op, log):
    """Copy the final slot, authority and observer evidence before fixture cleanup removes the workspace."""
    case.mkdir(parents=True)
    for name, src in (("anchor.json", slot.anchor), ("journal.json", slot.journal), ("health", slot.health)):
        if src.exists():
            shutil.copyfile(src, case / name)
    for name, src in (("candidates", op.candidates), ("authority", op.authority)):
        shutil.copytree(src, case / name)
    (case / "observed.json").write_bytes(canonical({
        "workspace": observe_tree(slot.workspace), "control": observe_tree(slot.control),
        "store": observe_tree(slot.store), "commands": log, "operator_commands": op.commands}))


def kill_stopped(child):
    """Kill the child at its qualification stop, require SIGKILL on reap and drain its pipes."""
    child.kill()
    assert child.wait(timeout=5) == -signal.SIGKILL
    child.communicate(timeout=3)


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
        f = Shipping(root, health, ARGS, self.inputs, self.profile, self.budget, content, "shipping-1")
        op = Operator(f.slot, root, health)
        dest = ARGS.evidence / "operator" / (self._testMethodName + ("-" + label if label else ""))
        self.addCleanup(save, dest, f.slot, op, f.commands)
        return f, op

    def converted(self, f, op):
        """Check that conversion preserves the source generation and attempts and binds both file digests
        to the new anchor.
        """
        old_anchor, old_journal = f.slot.anchor.read_bytes(), f.slot.journal.read_bytes()
        old = json.loads(old_journal)
        result = op.convert()
        new, anchor = read(f.slot.journal), read(f.slot.anchor)
        self.assertEqual(result["generation"], old["generation"])
        self.assertEqual(new["generation"], old["generation"])
        self.assertEqual(number_canonical(old["attempts"]), number_canonical(new["attempts"]))
        self.assertEqual(new["rulings"], [])
        self.assertEqual(anchor["journal_digest"], digest(f.slot.journal.read_bytes()))
        self.assertEqual(anchor["pending"], None)
        self.assertEqual(anchor["origin"], dict(source_generation=old["generation"],
            source_attempt_count=len(old["attempts"]), source_anchor_digest=digest(old_anchor),
            source_journal_digest=digest(old_journal), initial_journal_digest=anchor["journal_digest"]))
        self.assertEqual(new["reconciliation_profile_digest"], op.profile_digest)
        self.assertEqual(anchor["format"], "gaap.journal-anchor/0.2")
        self.assertEqual(new["format"], "gaap.journal/0.2")
        f.commands.append({"convert": result, "origin": anchor["origin"]})
        return old

    def test_conversion_preserves_originals(self):
        """Compare empty, partial and sealed v0.1 histories across conversion while watching the target for writes."""
        for kind in ("empty", "refused", "failed", "completed", "partial"):
            with self.subTest(kind=kind):
                bad = "def shipping_quote(quantity, unit_price):\n    return 7\n"
                f, op = self.fixture(kind, bad if kind == "failed" else GOOD)
                if kind == "refused":
                    f.proposal.update(path="deployment.json", content="{}")
                    update_grant(f.authority, proposal_digest=digest(canonical(f.proposal)))
                if kind == "partial":
                    with op.old_handoff(), f.stopped("target.before_rename") as child:
                        kill_stopped(child)
                elif kind != "empty":
                    with op.old_handoff():
                        code, sealed = f.submit(binary=ARGS.old_reference)
                        self.assertEqual((code, sealed["status"]), (0, "published"))
                        receipt = sealed["attempt"]["facts"][-1]["payload"]["receipt"]
                        self.assertEqual(receipt["body"]["terminal_status"] == "completed", kind == "completed")
                op.pin_source()
                status = "absent" if kind == "empty" else "partial" if kind == "partial" else "published"
                legacy = f.lookup(f.key, expected=status)
                with watch_target(f.workspace) as moves:
                    self.converted(f, op)
                    after = f.lookup(f.key, expected=status)
                    self.assertEqual(legacy.get("attempt"), after.get("attempt"))
                    self.assertEqual(after["workspace_hold"], legacy["workspace_hold"])
                    self.assertEqual(moves(), 0)

    def test_original_identity_reuse(self):
        """Duplicate an admission in a rehashed journal and require lookup to withhold even an unrelated key."""
        f, op = self.fixture()
        self.converted(f, op)
        code, result = f.submit()
        self.assertEqual((code, result["status"]), (0, "published"))
        journal = read(f.slot.journal)
        # Duplicate a whole frozen-valid envelope after its run completed, preserving
        # the untouched generation-zero origin. Rehash and confirm the otherwise valid pair.
        admission = journal["attempts"][0]
        journal["attempts"].append(copy.deepcopy(admission))
        journal["generation"] += len(admission["facts"])
        content = number_canonical(journal)
        anchor = read(f.slot.anchor)
        anchor.update(generation=journal["generation"], journal_digest=digest(content))
        write_private(f.slot.journal, content)
        write_private(f.slot.anchor, canonical(anchor))
        before = pair(f.slot), snapshot(f.target)
        denied = op.command("lookup", "unrelated", expected="invalid_store")
        self.assertNotIn("attempt", denied)
        self.assertEqual(before, (pair(f.slot), snapshot(f.target)))

    def test_legacy_sealed_unknown(self):
        """Check that conversion preserves an unknown effect's sealed receipt and keeps the workspace held."""
        self.root, self.health = self.area(ARGS.root), self.area(ARGS.health_root)
        self.slot = lambda **kwargs: Slot(self.root, self.health, ARGS.old_release, ARGS.qualification, **kwargs)
        self.record = lambda slot, name, result: setattr(self, "held_slot", slot)
        exercise_unknown(self, ARGS.old_reference, ARGS.evidence)
        slot = self.held_slot
        slot.release = ARGS.release
        prior = slot.call("lookup", "uncertain", expected="published")
        op = Operator(slot, self.root, self.health)
        history = prior["attempt"]
        result = op.convert()
        after = slot.call("lookup", "uncertain", expected="published")
        self.assertEqual(after["attempt"], history)
        self.assertTrue(after["workspace_hold"])
        self.assertEqual(history["facts"][-1]["payload"]["result"]["body"]["execution_status"], "unknown_outcome")
        save(ARGS.evidence / "operator" / self._testMethodName, slot, op,
             [{"convert": result, "unknown_receipt": history["facts"][-1]["payload"]["receipt"]["receipt_digest"]}])

    def test_mixed_writers(self):
        """Race a released writer against conversion and verify which history each client can use after the fence."""
        f, op = self.fixture("old-wins")
        with watch_target(f.workspace) as moves, op.old_handoff():
            code, old = f.submit(binary=ARGS.old_reference)
            self.assertEqual((code, old["status"], moves()), (0, "published", 1))
        prior = pair(f.slot)
        op.command("convert", expected="conflict")
        self.assertEqual(pair(f.slot), prior)
        f, op = self.fixture("cutover-wins")
        with watch_target(f.workspace) as moves:
            converter = f.slot.stop("cutover_fence.after_dir_sync", command=("convert",))
            self.addCleanup(terminate, converter)
            with op.old_handoff():
                children = [f.start(binary=ARGS.old_reference), f.slot.start(["enroll"], binary=ARGS.old_release)]
                for child in children:
                    self.addCleanup(terminate, child)
                children[0].stdin.write(canonical(f.proposal))
                children[0].stdin.close()
                children[0].stdin = None
                # Both old processes have opened their own v0.1 handoff before this owner dies.
                for child, exe in zip(children, (ARGS.old_reference, ARGS.old_release)):
                    limit = time.monotonic() + 2
                    while time.monotonic() < limit:
                        try:
                            if os.path.samefile(f"/proc/{child.pid}/exe", exe):
                                break
                        except FileNotFoundError:
                            pass
                        time.sleep(.002)
                    else:
                        self.fail("old client never reached its executable")
                kill_stopped(converter)
                for child in children:
                    code, reply = f.finish(child)
                    self.assertEqual((code != 0, reply["status"]), (True, "unsupported_storage"))
            op.command("lookup", f.key, expected="cutover_pending")
            code, held = f.submit()
            self.assertTrue(code and held["status"] == "cutover_pending", held)
            self.assertEqual(moves(), 0)
            self.assertEqual(op.convert()["generation"], 0)
            with op.old_handoff():
                op.command("lookup", f.key, binary=ARGS.old_release, expected="unsupported_storage")

    def test_conversion_interruptions(self):
        """Kill the converter around each rename and sync, then check the permitted retry against the
        surviving files.
        """
        for stage in ("cutover_fence", "cutover_journal", "cutover_active"):
            for cut in ("before_rename", "after_rename", "before_dir_sync", "after_dir_sync"):
                with self.subTest(stage=stage, cut=cut):
                    point = stage + "." + cut
                    f, op = self.fixture(point)
                    initial = snapshot(f.target)
                    child = f.slot.stop(point, command=("convert",))
                    kill_stopped(child)
                    anchor, journal = read(f.slot.anchor), read(f.slot.journal)
                    self.assertEqual(f.slot.health.read_text(), "0")
                    if cut == "before_rename":
                        directory = f.slot.store if stage == "cutover_journal" else f.slot.control
                        leftovers = [x for x in directory.iterdir() if x.name.startswith(".")]
                        self.assertTrue(leftovers)
                        before = observe_tree(directory)
                        op.command("convert", expected="invalid_store")
                        self.assertEqual(observe_tree(directory), before)
                    else:
                        self.assertEqual(anchor["phase"], "active" if stage == "cutover_active" else "cutover")
                        self.assertEqual(journal["format"], "gaap.journal/0.1" if stage == "cutover_fence" else "gaap.journal/0.2")
                        self.assertEqual(op.convert()["generation"], 0)
                    self.assertEqual(snapshot(f.target), initial)
                    f.commands.append({"cut": point, "observed_anchor": anchor, "observed_format": journal["format"]})
        f, op = self.fixture("lost-response")
        kill_stopped(f.slot.stop("convert.before_response", command=("convert",)))
        before = pair(f.slot)
        self.assertEqual(op.convert()["generation"], 0)
        self.assertEqual(before, pair(f.slot))

    def interrupted(self, kind, label):
        """Stop a real shipping worker and retain the observer's deposit alongside its dispatch-only history."""
        f, op = self.fixture(label)
        self.converted(f, op)
        entry = observe(f, op, kind)
        source = op.deposit(entry)
        original = f.lookup(f.key, expected="partial")["attempt"]
        self.assertEqual([v["tag"] for v in original["facts"]], ["intent", "dispatch"])
        f.commands.append({"observed_disposition": kind, "attempt_id": entry["attempt_id"],
                           "source": source, "pidfd": entry["stop"], "target": snapshot(f.target)})
        return f, op, entry, source, original

    def test_unchanged_sealed_failure_after_ruling(self):
        """Read the original failed receipt after a ruling and a duplicate submission, with the workspace
        under watch.
        """
        check_sealed_failure(self)

    def test_all_observed_outcomes_and_original_seals(self):
        """Admit each supported observation and verify that its interrupted run stays unsealed on lookup and retry."""
        for kind in ("not_started", "replaced", "stopped_before_rename"):
            with self.subTest(kind=kind):
                f, op, record, source, original = self.interrupted(kind, kind)
                with watch_target(f.workspace) as moves:
                    ruled = op.reconcile(record["attempt_id"], source)
                    self.assertEqual(ruled["ruling"]["disposition"], kind)
                    self.assertEqual(ruled["ruling"]["admitted_by"], "alice")
                    self.assertFalse(ruled["workspace_hold"])
                    after = f.lookup(f.key, expected="partial")
                    self.assertEqual(after["rulings"], [ruled["ruling"]])
                    self.assertEqual(after["attempt"], original)
                    self.assertEqual((f.submit()[1]["attempt"], moves()), (original, 0))

    def test_current_authority_and_incomplete_sources(self):
        """Check source eligibility, private readback and revocation between intake and the operator's
        final decision.
        """
        f, op, record, source, original = self.interrupted("replaced", "authority")
        attempt = record["attempt_id"]
        for right in ("read", "reconcile"):
            op.rights["alice"][right] = False
            op.publish_rights()
            state = pair(f.slot)
            known = op.reconcile(attempt, source, expected="forbidden")
            foreign = op.reconcile("0"*32, digest(b"unknown"), expected="forbidden")
            self.assertEqual(known, foreign)
            self.assertEqual(state, pair(f.slot))
            op.rights["alice"][right] = True
        op.publish_rights()
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
            refused = op.reconcile(attempt, op.deposit(bad), expected="ineligible_evidence")
            self.assertNotIn("ruling", refused)
            self.assertTrue(f.lookup(f.key, expected="partial")["workspace_hold"])
        cmd = ["reconcile", attempt, source]
        child = f.slot.stop("reconcile.before_authority", command=cmd)
        self.addCleanup(terminate, child)
        op.rights["alice"]["reconcile"] = False
        op.publish_rights()
        child.send_signal(signal.SIGCONT)
        code, denied = f.finish(child)
        self.assertTrue(code and denied["status"] == "forbidden", denied)
        self.assertEqual(read(f.slot.anchor)["pending"], dict(attempt_id=attempt, evidence_digest=source, intake_by="alice"))
        f.slot.host["principal"] = "operator2"
        f.slot.configure()
        saved = op.reconcile(attempt, source)
        self.assertEqual(saved["ruling"]["admitted_by"], "operator2")
        f.slot.host["principal"] = "bob"
        f.slot.configure()
        absent = f.lookup(f.key, expected="absent")
        self.assertFalse({"attempt", "rulings", "effect_status", "intake_pending"} & absent.keys())
        op.reconcile(attempt, source, expected="forbidden")

    def test_pending_crash_matrix_and_duplicate_operators(self):
        """Interrupt each ruling publication step and check what a second operator can recover from the files."""
        for stage in ("intake", "ruling", "confirm_ruling", "clear_intake"):
            for cut in ("before_rename", "after_rename", "before_dir_sync", "after_dir_sync"):
                with self.subTest(stage=stage, cut=cut):
                    point = stage + "." + cut
                    f, op, record, source, original = self.interrupted("not_started", point)
                    cmd = ["reconcile", record["attempt_id"], source]
                    kill_stopped(f.slot.stop(point, command=cmd))
                    anchor, journal = read(f.slot.anchor), read(f.slot.journal)
                    self.assertEqual(f.slot.health.read_text(), "0")
                    self.assertEqual(journal["attempts"], [original])
                    f.commands.append({"cut": point, "anchor": anchor, "journal_generation": journal["generation"]})
                    if cut == "before_rename":
                        state = pair(f.slot)
                        op.command(*cmd, expected="invalid_store")
                        self.assertEqual(pair(f.slot), state)
                    elif stage == "ruling":
                        self.assertEqual(journal["generation"], anchor["generation"] + 1)
                        self.assertEqual(anchor["pending"]["evidence_digest"], source)
                        op.command(*cmd, expected="unconfirmed")
                        op.command("lookup", f.key, expected="unconfirmed")
                    else:
                        if stage != "clear_intake":
                            self.assertEqual(anchor["pending"]["evidence_digest"], source)
                        saved = op.reconcile(*cmd[1:])
                        stable = pair(f.slot)
                        self.assertEqual(saved, op.reconcile(*cmd[1:]))
                        self.assertEqual(pair(f.slot), stable)
                        self.assertEqual(len(read(f.slot.journal)["rulings"]), 1)
                        self.assertIsNone(read(f.slot.anchor)["pending"])
        f, op, entry, source, _ = self.interrupted("not_started", "lost-response")
        cmd = ["reconcile", entry["attempt_id"], source]
        child = f.slot.stop("reconcile.before_response", command=cmd)
        self.addCleanup(terminate, child)
        competitor = f.slot.start(cmd)
        self.addCleanup(terminate, competitor)
        kill_stopped(child)
        code, result = f.finish(competitor)
        self.assertEqual((code, result["status"]), (0, "recorded"))
        self.assertEqual(len(read(f.slot.journal)["rulings"]), 1)
        self.assertEqual(result, op.reconcile(*cmd[1:]))

    def fresh(self, f, key):
        """Bind a new submission key and grant to the current workspace subject and the revised proposal."""
        f.key = key
        f.proposal.update(base_digest=subject(f.workspace), content=GOOD + "# separately authorized\n")
        approval = dict(f.grant["approval"], subject_digest=f.proposal["base_digest"], approval_id=key + "-grant")
        update_grant(f.authority, principal="alice", key=key, revoked=False,
                     proposal_digest=digest(canonical(f.proposal)), approval=approval)

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
        f, op, record, source, original = self.interrupted("replaced", "equivocation")
        attempt = record["attempt_id"]
        original_key = f.key
        with watch_target(f.workspace) as moves:
            first = op.reconcile(attempt, source)
            self.assertFalse(first["workspace_hold"])
            self.assertEqual(moves(), 0)
            self.fresh(f, "new-job")
            code, sealed = f.submit()
            self.assertEqual((code, sealed["status"], moves()), (0, "published", 1))
            receipt = sealed["attempt"]["facts"][-1]["payload"]["receipt"]
            self.assertEqual(receipt["body"]["terminal_status"], "completed")
            contradictory = op.deposit(self.equivocation(record))
            f.commands.append({"simulated_observer_equivocation": contradictory, "genuine_source": source})
            later = op.reconcile(attempt, contradictory)
            self.assertEqual(later["effect_status"], "disputed")
            self.assertTrue(later["workspace_hold"])
            repeat = op.reconcile(attempt, source)
            self.assertEqual(repeat["ruling"], first["ruling"])
            self.assertEqual(repeat["generation"], later["generation"])
            self.assertEqual(repeat["effect_status"], "disputed")
            self.assertEqual(f.lookup(original_key, expected="partial")["attempt"], original)
            self.assertEqual(f.lookup(f.key, expected="published")["attempt"], sealed["attempt"])
            self.assertEqual(f.submit()[1]["attempt"], sealed["attempt"])
            self.assertEqual(f.submit(key="unseen")[1]["status"], "workspace_held")
            self.assertEqual(f.submit(stdin=canonical({**f.proposal, "plan":"changed"}))[1]["status"], "conflict")
            self.assertEqual(moves(), 0)
        journal = read(f.slot.journal)
        self.assertEqual([r["original_fact_count"] for r in journal["rulings"]], [2, 7])
        self.assertEqual([r["recorded_generation"] for r in journal["rulings"]], [3, 9])
        # An authenticated file can still tell an impossible story. Move the second ruling
        # inside the later attempt and rehash the pair so the timeline itself must reject it.
        journal["rulings"][-1]["original_fact_count"] -= 1
        content = number_canonical(journal)
        anchor = read(f.slot.anchor)
        anchor["journal_digest"] = digest(content)
        write_private(f.slot.journal, content)
        write_private(f.slot.anchor, canonical(anchor))
        hidden = op.command("lookup", "unrelated", expected="invalid_store")
        self.assertNotIn("attempt", hidden)

    def test_quiescence_archive_loss_and_pinned_malformed_input(self):
        """Check that a surviving writer lease, missing evidence or an incomplete entry-gate record
        prevents unsafe progress.
        """
        for label in ("retained-writer", "lost-source", "malformed"):
            with self.subTest(label=label):
                f, op, rec, source, _ = self.interrupted("not_started", label)
                attempt = rec["attempt_id"]
                if label == "retained-writer":
                    with op.writers.open("rb") as lease:
                        fcntl.flock(lease, fcntl.LOCK_SH)
                        holder = subprocess.Popen(["cat"], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                                                  pass_fds=(lease.fileno(),))
                    try:
                        state = pair(f.slot)
                        op.reconcile(attempt, source, expected="busy")
                        self.assertEqual(pair(f.slot), state)
                    finally:
                        holder.communicate(timeout=4)
                    op.reconcile(attempt, source)
                elif label == "lost-source":
                    op.reconcile(attempt, source)
                    path = op.candidates / (source[7:] + ".json")
                    raw = path.read_bytes()
                    path.unlink()
                    self.assertNotIn("attempt", op.command("lookup", "unknown", expected={"invalid_store", "unavailable"}))
                    self.assertNotEqual(f.submit(key="fresh")[0], 0)
                    write_private(path, raw)
                    self.assertFalse(f.lookup(f.key, expected="partial")["workspace_hold"])
                else:
                    malformed = copy.deepcopy(rec)
                    del malformed["gate"]["reply"]
                    bad = op.deposit(malformed)
                    op.reconcile(attempt, bad, expected="invalid_store")
                    self.assertEqual(read(f.slot.anchor)["pending"]["evidence_digest"], bad)
                    self.assertTrue(f.lookup(f.key, expected="partial")["workspace_hold"])
                    op.reconcile(attempt, source, expected="busy")
                    self.assertEqual(f.submit(key="fresh")[1]["status"], "workspace_held")

    def test_capacity_and_storage_withdrawal(self):
        """Exhaust the generation counter and verify that the pending contradiction survives volume
        withdrawal and reopening.
        """
        f, op = self.fixture("exhausted-generation")
        with op.old_handoff():
            code, original = f.submit(binary=ARGS.old_reference)
            self.assertEqual(code, 0)
            legacy = read(f.slot.journal)
            legacy["generation"] = 2**53 - 6
            original_bytes = number_canonical(legacy)
            anchor = read(f.slot.anchor)
            anchor.update(generation=legacy["generation"], journal_digest=digest(original_bytes))
            write_private(f.slot.journal, original_bytes)
            write_private(f.slot.anchor, canonical(anchor))
            self.assertEqual(f.lookup(f.key, expected="published")["attempt"], original["attempt"])
        op.pin_source()
        self.converted(f, op)
        self.fresh(f, "last-key")
        corroborations = []
        rec = observe(f, op, "replaced", corroborations=corroborations)
        attempt = rec["attempt_id"]
        self.assertEqual(len(corroborations), 3)
        for sample in corroborations:
            op.reconcile(attempt, op.deposit(sample))
        self.assertEqual(read(f.slot.journal)["generation"], 2**53 - 1)
        disagreement = op.deposit(self.equivocation(rec))
        before = snapshot(f.slot.journal)
        op.reconcile(attempt, disagreement, expected="capacity")
        self.assertEqual(read(f.slot.anchor)["pending"]["evidence_digest"], disagreement)
        self.assertEqual(before, snapshot(f.slot.journal))
        self.assertTrue(f.lookup(f.key, expected="partial")["intake_pending"])
        for health in (b"P", b"1", b"0"):
            write_private(f.slot.health, health)
            if health != b"0":
                op.command("lookup", f.key, expected="unavailable")
            else:
                op.reconcile(attempt, disagreement, expected="capacity")
            self.assertEqual(read(f.slot.anchor)["pending"]["evidence_digest"], disagreement)

    def test_control_and_archive_failures(self):
        """Check the surviving target and operator response after writeback failure, a foreign stage, an
        unconfirmed journal or lock replacement.
        """
        for label in ("directory-sync", "stage-close", "foreign-stage", "one-ahead", "changed-lock"):
            with self.subTest(label=label):
                f, op = self.fixture(label)
                before = (snapshot(f.target), snapshot(f.slot.journal))
                if label in ("directory-sync", "stage-close"):
                    if label == "directory-sync":
                        other, baseline = self.fixture("calibrate")
                        trace = ARGS.evidence / "operator-sync-calibration.log"
                        baseline.command("convert", expected="converted", trace=trace)
                        syncs = [line for line in trace.read_text().splitlines() if line.startswith("fsync(")]
                        ordinal = next(i for i, line in enumerate(syncs, 1) if f"<{other.slot.control}>" in line)
                        failed = ARGS.evidence / "operator-sync-eio.log"
                        op.command("convert", expected="unavailable", trace=failed, fail_sync_ordinal=ordinal)
                        self.assertTrue(any(f"<{f.slot.control}>" in line and "EIO" in line and "INJECTED" in line
                                            for line in failed.read_text().splitlines()))
                    else:
                        op.command("convert", expected="unavailable", point="cutover_fence.close", action="eio")
                    self.assertEqual(f.slot.health.read_text(), "1")
                    op.command("lookup", f.key, expected="unavailable")
                elif label == "foreign-stage":
                    stage = f.slot.control / ".foreign"
                    write_private(stage, b"another owner")
                    op.command("convert", expected="invalid_store")
                    self.assertEqual(stage.read_bytes(), b"another owner")
                elif label == "one-ahead":
                    with op.old_handoff(), f.stopped("intent.after_rename") as worker:
                        kill_stopped(worker)
                    op.command("convert", expected="unconfirmed")
                else:
                    f.slot.lock.rename(f.slot.control / "former-lock")
                    write_private(f.slot.lock, b"")
                    op.command("convert", expected={"invalid_store", "unavailable"})
                self.assertEqual(snapshot(f.target), before[0])


if __name__ == "__main__":
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("release", "qualification", "reference", "fault-reference", "old-release",
                   "old-reference", "root", "health-root", "evidence"):
        parser.add_argument("--" + option, required=True, type=Path)
    ARGS, extra = parser.parse_known_args()
    unittest.main(argv=[__file__, *extra], verbosity=2)

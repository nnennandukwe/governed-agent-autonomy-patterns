"""Expected surviving records and forbidden replays for the ADR 0009 model."""

from concurrent.futures import ThreadPoolExecutor
from dataclasses import replace
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Event
import unittest
import json

from recovery_model import Cut, Host, Refusal, Submission, Target, digest, initialize, inspect, publish


def stop_at(cut):
    """Interrupt exactly one scripted step without running any later steps."""
    def interrupt(point):
        """Stop when the host reaches the selected boundary."""
        if point == cut:
            raise Cut(point)

    return interrupt


class RecoveryTests(unittest.TestCase):
    """Use an independent target and a fresh journal for each scenario."""

    def prepare(self):
        """Create a new single-workspace model and issue the first exact grant."""
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        initialize(self.root)
        self.target = Target()
        self.host = Host(self.root, self.target)
        self.caller = "client-1"
        self.request = self.fresh("one")
        self.host.grant(self.caller, self.request)

    def fresh(self, suffix):
        """Construct a separately identified request for the currently observed input."""
        return Submission(key="key-" + suffix, run="run-" + suffix, attempt="attempt-" + suffix,
                          effect="effect-" + suffix, request_id="request-" + suffix,
                          authority="grant-" + suffix, subject=digest(self.target.content),
                          replacement="replacement-" + suffix)

    def send(self, **options):
        """Submit the first exact caller-bound request through the model."""
        return self.host.submit(self.caller, self.request, **options)

    def observed(self, count, value):
        """Check actual target commits and bytes independently of the host response."""
        self.assertEqual(len(self.target.commits), count)
        self.assertEqual(self.target.content, value)

    def test_cut_matrix_and_response_loss(self):
        """Restart at each cut and check surviving facts, independent writes and replay."""
        # cut, stage, recorded effect, verification, receipt status, target mutation count
        cases = [
            ("before_intent", None, None, None, None, 0),
            ("before_dispatch", "intent", "not_executed", None, None, 0),
            ("before_effect", "dispatch", "unknown", None, None, 0),
            ("after_effect", "dispatch", "unknown", None, None, 1),
            ("during_verification", "observed", "executed", None, None, 1),
            ("during_receipt", "verified", "executed", "passed", None, 1),
            ("before_response", "published", "executed", "passed", "completed", 1),
            ("during_cleanup", "published", "executed", "passed", "completed", 1),
        ]
        dispatch_images = []
        for cut, stage, effect, verification, status, count in cases:
            with self.subTest(cut=cut):
                self.prepare()
                with self.assertRaisesRegex(Cut, cut):
                    self.send(at=stop_at(cut))
                value = self.request.replacement if count else "original"
                self.observed(count, value)
                image = (self.root / "journal.json").read_bytes()
                if cut == "during_receipt":
                    staged = json.loads((self.root / "journal.next").read_text(encoding="utf-8"))
                    self.assertEqual(staged["records"][0]["receipt"]["status"], "completed")
                if stage == "dispatch":
                    dispatch_images.append(image)
                # Reopening the host loses in-memory grants. Historical lookup needs no grant.
                self.host = Host(self.root, self.target)
                history = inspect(self.root)["records"]
                if stage is None:
                    self.assertEqual(history, [])
                    self.host.grant(self.caller, self.request)
                    self.assertEqual(self.send()["receipt"]["status"], "completed")
                    self.observed(1, self.request.replacement)
                    continue
                record, = history
                self.assertEqual((record["stage"], record["effect"], record["verification"]),
                                 (stage, effect, verification))
                self.assertEqual(record["receipt"] and record["receipt"]["status"], status)
                self.assertEqual(record["reserved_dispatches"], int(stage != "intent"))
                self.assertEqual(self.send(), record)
                self.assertEqual((self.root / "journal.json").read_bytes(), image)
                self.observed(count, value)
        self.assertEqual(dispatch_images[0], dispatch_images[1])

    def test_duplicate_submissions_overlap(self):
        """Hold the first writer at dispatch while a second writer enters the same operation."""
        self.prepare()
        first_at_dispatch, second_entered = Event(), Event()

        def hold_dispatch(point):
            """Wait until the contender has entered submission before committing once."""
            if point == "before_effect":
                first_at_dispatch.set()
                self.assertTrue(second_entered.wait(5))

        def announce_contender(point):
            """Signal that the concurrent submission reached the writer lock boundary."""
            if point == "before_lock":
                second_entered.set()

        with ThreadPoolExecutor(max_workers=2) as pool:
            first = pool.submit(self.send, at=hold_dispatch)
            self.assertTrue(first_at_dispatch.wait(5))
            duplicate = pool.submit(self.send, at=announce_contender)
            self.assertEqual(first.result(timeout=5), duplicate.result(timeout=5))
        self.assertEqual(len(inspect(self.root)["records"]), 1)
        self.observed(1, self.request.replacement)

    def test_identity_conflict_and_caller_isolation(self):
        """Only matching immutable bytes and the original caller can retrieve the result."""
        self.prepare()
        original = self.send()
        changed = replace(self.request, replacement="changed")
        with self.assertRaisesRegex(Refusal, "Identity conflict"):
            self.host.submit(self.caller, changed)
        with self.assertRaisesRegex(Refusal, "fresh identities"):
            self.host.submit("client-2", self.request)
        fresh = self.fresh("two")
        for name in ("request_id", "run", "attempt", "effect", "authority"):
            with self.subTest(reused=name), self.assertRaisesRegex(Refusal, "fresh identities"):
                self.host.submit(self.caller, replace(fresh, **{name: getattr(self.request, name)}))
        self.assertEqual(self.send(), original)
        self.observed(1, self.request.replacement)

    def test_unresolved_workspace_and_reconciliation(self):
        """Require closed attributable evidence to resolve either side of the commit window."""
        for cut, script, expected, count in [
            ("before_effect", "normal", "not_executed", 0),
            ("after_effect", "normal", "executed", 1),
            ("after_effect", "executor_failure", "failed", 0),
        ]:
            with self.subTest(cut=cut, script=script):
                self.prepare()
                with self.assertRaises(Cut):
                    self.send(at=stop_at(cut), script=script)
                fresh = self.fresh("two")
                self.host.grant(self.caller, fresh)
                with self.assertRaisesRegex(Refusal, "Workspace unresolved"):
                    self.host.submit(self.caller, fresh)
                binding = self.request.binding(self.caller)
                args = ("host-reconciler", self.caller, self.request)
                with self.assertRaisesRegex(Refusal, "final independent witness"):
                    self.host.admit(*args, {"binding": binding, "outcome": expected})
                witness = self.target.fence(binding)
                with self.assertRaisesRegex(Refusal, "fenced"):
                    self.target.invoke(binding, self.request, False)
                before = inspect(self.root)
                with self.assertRaisesRegex(Refusal, "Only the host"):
                    self.host.admit("client-1", self.caller, self.request, witness)
                for invalid in ({}, {**witness, "binding": "another-attempt"},
                                {**witness, "outcome": "contradiction"}):
                    with self.assertRaisesRegex(Refusal, "final independent witness"):
                        self.host.admit(*args, invalid)
                self.assertEqual(inspect(self.root), before)
                ruled = self.host.admit(*args, witness)
                self.assertEqual(ruled["ruling"]["outcome"], expected)
                self.assertEqual((ruled["effect"], ruled["receipt"]), ("unknown", None))
                self.assertEqual(self.send(), self.host.admit(*args, witness))
                self.observed(count, self.request.replacement if count else "original")
                self.assertEqual(self.host.submit(self.caller, fresh)["receipt"]["status"],
                                 "completed")
                self.observed(count + 1, fresh.replacement)

    def test_failed_sealed_receipt_survives_later_ruling(self):
        """A later positive witness can explain an unknown effect in a sealed failed run."""
        self.prepare()
        sealed = self.send(script="unknown_report")
        self.assertEqual(sealed["receipt"], {"status": "failed", "reason": "unknown_outcome",
                                             "effect": "unknown"})
        witness = self.target.fence(self.request.binding(self.caller))
        ruled = self.host.admit("host-reconciler", self.caller, self.request, witness)
        self.assertEqual(ruled["ruling"]["outcome"], "executed")
        self.assertEqual(self.send()["receipt"], sealed["receipt"])
        self.observed(1, self.request.replacement)

    def test_cancel_failure_and_verification_preserve_effect(self):
        """Report the terminal run decision independently of the observed workspace change."""
        for script, status, effect, verification, count in [
            ("cancel_before_dispatch", "interrupted", "not_executed", None, 0),
            ("cancel_after_effect", "interrupted", "executed", None, 1),
            ("executor_failure", "failed", "failed", None, 0),
            ("verification_failure", "blocked", "executed", "failed", 1),
        ]:
            with self.subTest(script=script):
                self.prepare()
                record = self.send(script=script)
                self.assertEqual(record["receipt"]["status"], status)
                self.assertEqual((record["effect"], record["verification"]), (effect, verification))
                self.assertEqual(self.send(), record)
                self.observed(count, self.request.replacement if count else "original")

    def test_guard_changes_before_dispatch(self):
        """Recheck authority, configuration and input bytes after intent publication."""
        for drift in ("expired", "revoked", "policy", "capability", "verifier", "subject"):
            with self.subTest(drift=drift):
                self.prepare()

                def change_after_intent(point):
                    """Change a trusted input only after the submission identity has been saved."""
                    if point != "before_dispatch":
                        return
                    if drift in ("expired", "revoked"):
                        self.host.grants[self.request.authority] = (
                            self.request.binding(self.caller), drift
                        )
                    elif drift == "subject":
                        self.target.content = "outside edit"
                    else:
                        setattr(self.host, drift, "version-2")

                record = self.send(at=change_after_intent)
                self.assertEqual(record["receipt"]["status"], "blocked")
                self.assertEqual(record["reserved_dispatches"], 0)
                self.assertEqual(self.send(), record)
                self.observed(0, "outside edit" if drift == "subject" else "original")

    def test_drift_after_dispatch_but_before_target_commit(self):
        """A durable dispatch can't authorize a write after a relevant binding changes."""
        for drift in ("revoked", "capability", "policy", "subject", "cancel"):
            with self.subTest(drift=drift):
                self.prepare()

                def change_before_replace(point):
                    """Change one trusted precondition after the durable fence."""
                    if point != "before_effect":
                        return
                    if drift == "revoked":
                        self.host.grants[self.request.authority] = (
                            self.request.binding(self.caller), "revoked"
                        )
                    elif drift == "subject":
                        self.target.content = "outside edit"
                    elif drift != "cancel":
                        setattr(self.host, drift, "version-2")

                script = "cancel_before_effect" if drift == "cancel" else "normal"
                record = self.send(at=change_before_replace, script=script)
                self.assertEqual(record["effect"], "not_executed")
                self.assertEqual(record["receipt"]["status"],
                                 "interrupted" if drift == "cancel" else "blocked")
                self.assertEqual(record["reserved_dispatches"], 1)
                self.assertEqual(self.send(), record)
                self.observed(0, "outside edit" if drift == "subject" else "original")

    def test_verifier_binds_latest_observed_subject_through_sealing(self):
        """A verifier or caller can't complete a run against a superseded output."""
        for window, verdict in (("during_verification", "failed"), ("before_seal", "passed")):
            with self.subTest(window=window):
                self.prepare()

                def change_output(point):
                    """Replace the observed output before the verdict or before sealing."""
                    if point == window:
                        self.target.content = "later outside edit"

                record = self.send(at=change_output)
                self.assertEqual(record["effect"], "executed")
                self.assertEqual(record["observed_subject"], digest(self.request.replacement))
                self.assertEqual(record["verification"], verdict)
                self.assertEqual(record["receipt"]["status"], "blocked")
                self.assertEqual(self.send(), record)
                self.observed(1, "later outside edit")

    def test_one_shot_approval_and_history_during_uncertainty(self):
        """A new grant needs fresh identities; uncertainty elsewhere can't hide a known receipt."""
        self.prepare()
        self.host.grants.clear()
        sealed = self.send()
        self.assertEqual(sealed["receipt"]["reason"], "authority_missing")
        self.host.grant(self.caller, self.request)
        self.assertEqual(self.send(), sealed)
        fresh = replace(self.fresh("two"), replacement=self.request.replacement)
        old_proposal = json.loads(self.request.binding(self.caller))["proposal_digest"]
        self.assertEqual(json.loads(fresh.binding(self.caller))["proposal_digest"], old_proposal)
        self.host.grant(self.caller, fresh)
        with self.assertRaises(Cut):
            self.host.submit(self.caller, fresh, at=stop_at("after_effect"))
        image = (self.root / "journal.json").read_bytes()
        self.assertEqual(Host(self.root, self.target).submit(self.caller, self.request), sealed)
        self.assertEqual(inspect(self.root)["records"][0], sealed)
        self.assertEqual((self.root / "journal.json").read_bytes(), image)
        self.observed(1, fresh.replacement)

    def test_unsupported_schema_refuses_read_and_dispatch(self):
        """Refuse interpretation or replacement of an unsupported saved generation."""
        self.prepare()
        snapshot = inspect(self.root)
        snapshot["version"] = 2
        publish(self.root, snapshot)
        image = (self.root / "journal.json").read_bytes()
        with self.assertRaisesRegex(Refusal, "Unsupported journal version"):
            inspect(self.root)
        with self.assertRaisesRegex(Refusal, "Unsupported journal version"):
            self.send()
        self.assertEqual((self.root / "journal.json").read_bytes(), image)
        self.observed(0, "original")

    def test_cleanup_warning_preserves_receipt_and_unowned_state(self):
        """Failure to clean owned scratch leaves the known outcome and other owners intact."""
        self.prepare()
        self.target.scratch.add("other owner's stage")

        def fail_cleanup(point):
            """Raise an ordinary cleanup error after receipt publication."""
            if point == "during_cleanup":
                raise OSError("scripted failure")

        reply = self.send(at=fail_cleanup)
        self.assertIn("cleanup", reply["warning"])
        stable = self.send()
        self.assertEqual(stable["receipt"], reply["receipt"])
        self.assertNotIn("warning", stable)
        self.assertEqual(self.target.scratch, {"other owner's stage", self.request.binding(self.caller)})
        self.observed(1, self.request.replacement)


if __name__ == "__main__":
    unittest.main()

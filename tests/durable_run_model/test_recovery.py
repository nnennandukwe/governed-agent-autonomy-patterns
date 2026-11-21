"""Expected surviving records and forbidden replays for the ADR 0009 model."""

from concurrent.futures import ThreadPoolExecutor
from dataclasses import replace
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Event
from unittest.mock import patch
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

    def prepare(self, store_id="store-A", target_id="workspace-A"):
        """Create a new single-workspace model and issue the first exact grant."""
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.target = Target(self.root, store_id, target_id)
        initialize(self.root, self.target)
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
                binding = self.request.binding(self.caller, self.target.enrollment)
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

    def test_failed_executor_report_needs_an_independent_witness(self):
        """An executor can report failure both before and after a committed write."""
        for committed in (False, True):
            for witness_available in (False, True):
                with self.subTest(committed=committed, witness_available=witness_available):
                    self.prepare()
                    invoke = self.target.invoke
                    fence = self.target.fence

                    def report_failure(binding, request, fail):
                        """Report the same failure for two different target histories."""
                        invoke(binding, request, fail=not committed)
                        return "failed"

                    def observe(binding):
                        """Make the independent final observation available only in selected cases."""
                        return fence(binding) if witness_available else None

                    with patch.object(self.target, "invoke", report_failure), \
                            patch.object(self.target, "fence", observe):
                        record = self.send()
                    known = "executed" if committed else "failed"
                    self.assertEqual(record["effect"], known if witness_available else "unknown")
                    self.assertEqual(record["receipt"]["status"], "failed")
                    self.assertIsNone(record["verification"])
                    self.observed(int(committed), self.request.replacement if committed else "original")
                    if not witness_available:
                        with self.assertRaisesRegex(Refusal, "Workspace unresolved"):
                            self.host.submit(self.caller, self.fresh("two"))
                        witness = fence(record["binding"])
                        ruled = self.host.admit("host-reconciler", self.caller, self.request, witness)
                        self.assertEqual(ruled["ruling"]["outcome"], known)
                        self.assertEqual(ruled["receipt"], record["receipt"])

    def test_target_checks_the_original_invocation_and_output_subject(self):
        """A changed payload can't use a registered invocation or fabricate output evidence."""
        self.prepare()
        with self.assertRaises(Cut):
            self.send(at=stop_at("before_effect"))
        binding = self.send()["binding"]
        altered = replace(self.request, replacement="unapproved bytes")
        with self.assertRaisesRegex(Refusal, "Invocation bytes conflict"):
            self.target.invoke(binding, altered, False)
        self.observed(0, "original")
        self.target.invoke(binding, self.request, False)
        witness = self.target.fence(binding)
        self.assertEqual(witness["output_subject"], digest(self.request.replacement))
        wrong_subject = {**witness, "output_subject": digest("unapproved bytes")}
        with self.assertRaisesRegex(Refusal, "final independent witness"):
            self.host.admit("host-reconciler", self.caller, self.request, wrong_subject)
        # Even a configured witness source must attest to the authorized output.
        with patch.dict(self.target.witnesses, {binding: wrong_subject}):
            with self.assertRaisesRegex(Refusal, "final independent witness"):
                self.host.admit("host-reconciler", self.caller, self.request, wrong_subject)
        ruled = self.host.admit("host-reconciler", self.caller, self.request, witness)
        self.assertEqual(ruled["ruling"]["output_subject"], digest(self.target.content))
        self.observed(1, self.request.replacement)

    def test_cross_enrollment_evidence_and_reopen_are_refused(self):
        """Bind otherwise identical requests and observations to their enrolled store and target."""
        self.prepare()
        with self.assertRaises(Cut):
            self.send(at=stop_at("after_effect"))
        foreign_target = self.target
        foreign_witness = foreign_target.fence(self.send()["binding"])
        for store_id, target_id in (("store-B", "workspace-A"), ("store-A", "workspace-B")):
            with self.subTest(store_id=store_id, target_id=target_id):
                self.prepare(store_id, target_id)
                with self.assertRaises(Cut):
                    self.send(at=stop_at("after_effect"))
                local_witness = self.target.fence(self.send()["binding"])
                image = (self.root / "journal.json").read_bytes()
                with self.assertRaisesRegex(Refusal, "enrollment mismatch"):
                    Host(self.root, foreign_target)
                with self.assertRaisesRegex(Refusal, "final independent witness"):
                    self.host.admit("host-reconciler", self.caller, self.request, foreign_witness)
                self.assertEqual((self.root / "journal.json").read_bytes(), image)
                self.assertEqual(local_witness["output_subject"], foreign_witness["output_subject"])
                self.assertEqual(self.host.admit("host-reconciler", self.caller, self.request,
                                                 local_witness)["ruling"]["outcome"], "executed")
                self.observed(1, self.request.replacement)

    def test_reused_identity_conflicts_during_an_unresolved_attempt(self):
        """Reject reused host identities before evaluating the workspace-wide hold."""
        self.prepare()
        with self.assertRaises(Cut):
            self.send(at=stop_at("before_effect"))
        fresh = self.fresh("two")
        for field in ("run", "attempt", "effect", "request_id", "authority"):
            with self.subTest(field=field), self.assertRaisesRegex(Refusal, "Identity conflict"):
                self.host.submit(self.caller, replace(fresh, **{field: getattr(self.request, field)}))
        with self.assertRaisesRegex(Refusal, "Workspace unresolved"):
            self.host.submit(self.caller, fresh)
        self.observed(0, "original")

    def test_conflicting_trusted_witnesses_hold_the_workspace(self):
        """Keep both observations and the sealed receipt when a trusted source contradicts itself."""
        self.prepare()
        receipt = self.send(script="unknown_report")["receipt"]
        binding = self.request.binding(self.caller, self.target.enrollment)
        args = ("host-reconciler", self.caller, self.request)
        original = self.target.fence(binding)
        prior = self.host.admit(*args, original)
        contrary = {**original, "outcome": "failed", "output_subject": None}
        self.target.witnesses[binding] = contrary
        for _ in range(2):
            with self.assertRaisesRegex(Refusal, "Conflicting evidence"):
                self.host.admit(*args, contrary)
        self.host = Host(self.root, self.target)
        saved = self.send()
        self.assertEqual(saved["receipt"], receipt)
        self.assertEqual(saved["ruling"], prior["ruling"])
        self.assertEqual(saved["dispute"], {"actor": "host-reconciler", **contrary})
        fresh = self.fresh("two")
        self.host.grant(self.caller, fresh)
        with self.assertRaisesRegex(Refusal, "Workspace unresolved"):
            self.host.submit(self.caller, fresh)
        self.observed(1, self.request.replacement)

    def test_failed_sealed_receipt_survives_later_ruling(self):
        """A later positive witness can explain an unknown effect in a sealed failed run."""
        self.prepare()
        sealed = self.send(script="unknown_report")
        self.assertEqual(sealed["receipt"], {"status": "failed", "reason": "unknown_outcome",
                                             "effect": "unknown"})
        witness = self.target.fence(self.request.binding(self.caller, self.target.enrollment))
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
                            self.request.binding(self.caller, self.target.enrollment), drift
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
                            self.request.binding(self.caller, self.target.enrollment), "revoked"
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
        old_proposal = json.loads(self.request.binding(self.caller, self.target.enrollment))["proposal_digest"]
        self.assertEqual(json.loads(fresh.binding(self.caller, self.target.enrollment))["proposal_digest"], old_proposal)
        self.host.grant(self.caller, fresh)
        with self.assertRaises(Cut):
            self.host.submit(self.caller, fresh, at=stop_at("after_effect"))
        image = (self.root / "journal.json").read_bytes()
        self.assertEqual(Host(self.root, self.target).submit(self.caller, self.request), sealed)
        self.assertEqual(inspect(self.root)["records"][0], sealed)
        self.assertEqual((self.root / "journal.json").read_bytes(), image)
        self.observed(1, fresh.replacement)

    def test_malformed_binding_refuses_first_admission(self):
        """An invalid caller or request can't publish an unreadable intent or change the target."""
        self.prepare()
        path = self.root / "journal.json"
        intact = path.read_bytes()
        for caller, request in [
            (self.caller, replace(self.request, key="")),
            (self.caller, replace(self.request, run=1)),
            ("", self.request),
        ]:
            with self.subTest(caller=caller, key=request.key, run=request.run):
                self.host.grant(caller, request)
                with self.assertRaisesRegex(Refusal, "canonical binding"):
                    self.host.submit(caller, request)
                self.assertEqual(path.read_bytes(), intact)
                self.assertEqual(inspect(self.root)["records"], [])
                self.observed(0, "original")
        self.host.grant(self.caller, self.request)
        self.assertEqual(self.send()["receipt"]["status"], "completed")

    def test_damaged_journal_refuses_history_and_fresh_dispatch(self):
        """Keep the workspace closed when a surviving dispatch generation is damaged."""
        from recovery_model import journal_bytes

        self.prepare()
        with self.assertRaises(Cut):
            self.send(at=stop_at("after_effect"))
        path = self.root / "journal.json"
        intact = inspect(self.root)
        fresh = self.fresh("two")
        self.host.grant(self.caller, fresh)
        erased = json.loads(path.read_text(encoding="utf-8"))
        erased["records"] = []
        bad_receipt = {"effect": "unknown", "status": "completed", "reason": "verified"}
        cases = {
            "checksum after erasing history": json.dumps(erased, separators=(",", ":"), sort_keys=True),
            "malformed JSON": "{",
            "wrong enrollment": journal_bytes({**intact, "enrollment": {}}),
            "duplicate identity": journal_bytes({**intact, "records": intact["records"] * 2}),
        }
        for label, changes in [
            ("unknown disposition", {"effect": "damaged"}),
            ("impossible dispatch", {"effect": "failed"}),
            ("unknown stage", {"stage": "resumed"}),
            ("mismatched caller key", {"key": fresh.key}),
            ("missing attempt binding", {"binding": "{}"}),
            ("malformed reservation", {"reserved_dispatches": True}),
            ("premature verdict", {"verification": "passed"}),
            ("invented completion", {"stage": "published", "receipt": bad_receipt}),
            ("unsupported ruling", {"ruling": {}}),
        ]:
            tampered = {**intact["records"][0], **changes}
            cases[label] = journal_bytes({**intact, "records": [tampered]})
        for label, image in cases.items():
            with self.subTest(label=label):
                path.write_text(image, encoding="utf-8")
                with self.assertRaises(Refusal):
                    inspect(self.root)
                with self.assertRaises(Refusal):
                    self.host.submit(self.caller, fresh)
                self.assertEqual(path.read_text(encoding="utf-8"), image)
                self.observed(1, self.request.replacement)

    def test_unsupported_schema_refuses_read_and_dispatch(self):
        """Refuse interpretation or replacement of an unsupported saved generation."""
        self.prepare()
        snapshot = inspect(self.root)
        for version in (2, True, 1.0, "1"):
            with self.subTest(version=repr(version)):
                snapshot["version"] = version
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
        self.assertEqual(self.target.scratch, {"other owner's stage", self.request.binding(self.caller, self.target.enrollment)})
        self.observed(1, self.request.replacement)


if __name__ == "__main__":
    unittest.main()

"""Exercise offline rehearsal and independent verification through the host CLI."""

import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import corpus
import recorder


HERE = Path(__file__).resolve().parent


def cli(*args):
    """Invoke the host command with the same interpreter as the test process."""
    return subprocess.run(
        [sys.executable, "-B", str(HERE / "recorder.py"), *map(str, args)],
        capture_output=True, text=True, check=False,
    )


class RecorderTest(unittest.TestCase):
    """Check the packet from a new scratch directory for each scenario."""

    def test_public_controls_and_unrun_provider_assignments(self):
        """A complete packet reports observed controls and keeps every provider cell unrun."""
        with tempfile.TemporaryDirectory() as directory:
            packet = Path(directory) / "packet"
            created = cli("rehearse", packet)
            self.assertEqual(created.returncode, 0, created.stderr)
            root = created.stdout.strip()
            verified = cli("verify", packet, "--expected-root", root)
            self.assertEqual(verified.returncode, 0, verified.stderr)
            summary = json.loads(verified.stdout)
            self.assertEqual(summary["provider_study"]["assigned"], 48)
            self.assertEqual(summary["provider_study"]["attempted"], 0)
            self.assertEqual(summary["provider_study"]["verified_rate"], None)
            self.assertEqual(summary["development_controls"]["assigned"], 8)
            self.assertGreater(summary["development_controls"]["outcomes"]["verified"], 0)
            self.assertGreater(summary["development_controls"]["outcomes"]["rejected"], 0)
            self.assertGreater(summary["development_controls"]["outcomes"]["missing_proposal"], 0)

    def test_selection_and_revision_evidence(self):
        """A passing decoy wins in order, while lesson and revision retain their failed parent."""
        with tempfile.TemporaryDirectory() as directory:
            packet = Path(directory) / "packet"
            root = recorder.rehearse(packet)
            first_pass = recorder.read(packet / "control-3.json")
            self.assertEqual([item["public_pass"] for item in first_pass["candidates"]], [True, True])
            self.assertEqual(first_pass["selected"], 0)
            self.assertEqual(first_pass["status"], "rejected")
            self.assertTrue(first_pass["evaluation"]["effect_valid"])
            self.assertEqual(first_pass["usage"]["strategy_tool_calls"], 4)
            requests = [recorder.read(packet / f"control-{index}.json")["initial_request_utf8"] for index in range(4)]
            self.assertEqual(len(set(requests)), 1)
            self.assertEqual([item["request_utf8"] for item in first_pass["candidates"]], [requests[0]] * 2)
            claims = recorder.read(packet / "summary.json")["development_controls"]["scripted_completion_claims"]
            self.assertEqual((claims["claimed"], claims["unknown"], claims["false_completions"]), (7, 1, 1))
            self.assertEqual((claims["per_attempt"], claims["per_claim"]), (1/8, 1/7))
            for index, arm in ((1, "revision"), (2, "lesson")):
                record = recorder.read(packet / f"control-{index}.json")
                first, revision = record["candidates"]
                self.assertEqual(record["arm"], arm)
                self.assertIs(first["public_pass"], False)
                self.assertEqual(revision["parent"], 0)
                self.assertEqual(record["selected"], 1)
                if arm == "lesson":
                    self.assertEqual(record["lesson"]["sha256"], first["sha256"])
                    self.assertEqual(record["lesson"]["member"], record["member"])
                else:
                    self.assertIsNone(record["lesson"])
            self.assertEqual(recorder.verify(packet, root)["provider_study"]["trial_cost_microusd"], None)

    def test_caps_and_no_passing_candidate(self):
        """The host accounts for every sample, refuses excess work and keeps the original subject."""
        task, gold, decoy = corpus.controls()[0]
        limits = recorder.read(HERE / "budget.json")["per_cell_limits"]
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / "control.json"
            both_bad = recorder.run_control(task, [recorder.fixture_response(x, False) for x in (b"{}", b'{"extra":1}')], "resampling", limits, record)
            self.assertEqual(both_bad["selected"], 0)
            self.assertFalse(both_bad["evaluation"]["correct"])
            self.assertEqual(both_bad["usage"]["evaluator_calls"], 1)
            for key in ("candidates", "strategy_tool_calls", "evaluator_calls", "elapsed_seconds"):
                with self.subTest(key=key):
                    bounded = recorder.run_control(task, [recorder.fixture_response(x, True) for x in (decoy, gold)], "resampling",
                                                   dict(limits, **{key: 0}), record)
                    initial = [recorder.fixture_response(x, True) for x in (decoy, gold)]
                    recorder.verify_control(task, "resampling", initial, bounded, dict(limits, **{key: 0}))
                    self.assertEqual(bounded["reason"], key)
                    self.assertEqual(bounded["status"], "timeout" if key == "elapsed_seconds" else "exhausted")
                    self.assertIsNone(bounded["evaluation"])
                    self.assertEqual(bounded["usage"]["provider_calls"], 0)
                    self.assertEqual(bounded["usage"]["total_tokens"], 0)
                    if key == "evaluator_calls":
                        self.assertEqual(bounded["effect"], "observed")
                    else:
                        self.assertEqual(bounded["before"], bounded["after"])

    def test_external_root_and_frozen_source(self):
        """An independent verifier rejects altered source, substituted summaries and extra files."""
        with tempfile.TemporaryDirectory() as directory:
            root_dir = Path(directory)
            packet = root_dir / "packet"
            digest = recorder.rehearse(packet)
            source = root_dir / "source"
            shutil.copytree(HERE, source, ignore=shutil.ignore_patterns("__pycache__"))
            with (source / "study.json").open("ab") as stream:
                stream.write(b"\n")
            checked = subprocess.run(
                [sys.executable, "-B", str(source / "recorder.py"), "verify", str(packet),
                 "--expected-root", digest], capture_output=True, text=True, check=False,
            )
            self.assertNotEqual(checked.returncode, 0)
            self.assertIn("Source bytes", checked.stderr)
            (packet / "extra").touch()
            with self.assertRaisesRegex(ValueError, "Unexpected or missing"):
                recorder.verify(packet, digest)
            (packet / "extra").unlink()
            recorder.save(packet / "summary.json", {"trial_won": True})
            with self.assertRaisesRegex(ValueError, "supplied root"):
                recorder.verify(packet, digest)

    def test_verifier_parses_the_authenticated_snapshot(self):
        """A disk replacement after hashing can't change the returned verification result."""
        with tempfile.TemporaryDirectory() as directory:
            packet = Path(directory) / "packet"
            digest = recorder.rehearse(packet)
            original = recorder.read(packet / "summary.json")
            hash_bytes = recorder.root_digest

            def replace_after_hash(contents):
                """Change a file after computing the root over the verifier's captured bytes."""
                observed = hash_bytes(contents)
                (packet / "control-0.json").write_bytes(b"not JSON")
                return observed

            with patch.object(recorder, "root_digest", replace_after_hash):
                self.assertEqual(recorder.verify(packet, digest), original)
            with self.assertRaisesRegex(ValueError, "supplied root"):
                recorder.verify(packet, digest)

    def test_interruption_after_replace_preserves_the_unscored_selection(self):
        """A process killed after its target write leaves an incomplete packet and can't resume."""
        with tempfile.TemporaryDirectory() as directory:
            packet = Path(directory) / "packet"
            program = """
import os
import signal
from pathlib import Path
import sys
import recorder
replace = os.replace
def cut_after_target(source, target):
    '''Exit as soon as the actual target replacement succeeds.'''
    replace(source, target)
    if Path(target).name == 'settings.json':
        os.kill(os.getpid(), signal.SIGKILL)
os.replace = cut_after_target
recorder.rehearse(Path(sys.argv[1]))
"""
            stopped = subprocess.run([sys.executable, "-B", "-c", program, str(packet)],
                                     cwd=HERE, capture_output=True, check=False,
                                     env=dict(os.environ, TMPDIR=directory))
            self.assertEqual(stopped.returncode, -signal.SIGKILL, stopped.stderr)
            record = recorder.read(packet / "control-0.json")
            self.assertEqual(record["effect"], "uncertain")
            self.assertIsNone(record["evaluation"])
            self.assertEqual(record["selected"], 0)
            target = list(Path(directory).glob("tmp*/selected/settings.json"))
            self.assertEqual(len(target), 1)
            self.assertEqual(target[0].read_bytes(), record["candidates"][0]["utf8"].encode())
            self.assertFalse((packet / "summary.json").exists())
            self.assertNotEqual(cli("verify", packet, "--expected-root", "0" * 64).returncode, 0)
            self.assertNotEqual(cli("rehearse", packet).returncode, 0)
            self.assertEqual(recorder.read(packet / "control-0.json"), record)

    def test_malformed_refused_and_unrecognized_record_state(self):
        """Bad or refused output can't earn a score and an unknown state invalidates verification."""
        task, _, _ = corpus.controls()[0]
        limits = recorder.read(HERE / "budget.json")["per_cell_limits"]
        refusal = recorder.encoded({"replacement_utf8": None, "claimed_complete": False,
                                   "refusal": "No replacement.", "lesson": None})
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "one.json"
            for reply, status in ((refusal, "refused"), (b"false", "malformed_response")):
                record = recorder.run_control(task, [reply], "raw", limits, output)
                self.assertEqual(record["status"], status)
                self.assertEqual(record["effect"], "none")
                self.assertEqual(record["evaluation"], None)
                self.assertEqual(record["before"], record["after"])
                self.assertEqual(record["candidates"][0]["reply_hex"], reply.hex())
            packet = Path(directory) / "packet"
            recorder.rehearse(packet)
            first = recorder.read(packet / "control-0.json")
            for field, value, message in (("status", "congratulations", "Incomplete control"),
                                          ("usage", {**first["usage"], "elapsed_ns": -1}, "nonnegative"),
                                          ("usage", {**first["usage"], "candidates": True}, "nonnegative")):
                record = dict(first, **{field: value})
                recorder.save(packet / "control-0.json", record)
                digest = recorder.root_digest(recorder.packet_bytes(packet, [p.name for p in packet.iterdir()]))
                with self.assertRaisesRegex(ValueError, message):
                    recorder.verify(packet, digest)

    def test_late_oracle_retained_and_verified_without_score_credit(self):
        """Keep a late verdict and its incurred charge, then independently verify its content."""
        with tempfile.TemporaryDirectory() as directory:
            packet = Path(directory) / "packet"
            monotonic = recorder.time.monotonic_ns
            evaluate = corpus.evaluate
            offset = [0]

            def finish_after_deadline(*args):
                """Advance the recorded clock after the first real evaluator returns."""
                result = evaluate(*args)
                if not offset[0]:
                    offset[0] = 301_000_000_000
                return result

            with patch.object(recorder.time, "monotonic_ns", side_effect=lambda: monotonic() + offset[0]), \
                 patch.object(corpus, "evaluate", side_effect=finish_after_deadline):
                root = recorder.rehearse(packet)
            summary = recorder.verify(packet, root)["development_controls"]
            first = recorder.read(packet / "control-0.json")
            self.assertEqual(first["status"], "timeout")
            self.assertEqual(first["stopped_before"], "finish")
            self.assertTrue(first["evaluation"]["correct"])
            self.assertEqual(summary["usage"]["evaluator_calls"], 7)
            self.assertEqual(summary["retained_late_evaluations"], 1)
            self.assertEqual((summary["scored"], summary["unscored"]), (6, 2))
            first["evaluation"]["correct"] = False
            recorder.save(packet / "control-0.json", first)
            sealed = recorder.root_digest(recorder.packet_bytes(packet, [p.name for p in packet.iterdir()]))
            checked = cli("verify", packet, "--expected-root", sealed)
            self.assertNotEqual(checked.returncode, 0)
            self.assertIn("retained evaluation", checked.stderr)

    def test_unscored_placement_checks_target_and_charges(self):
        """Reconcile an admitted replacement and its operations even if scoring couldn't start."""
        task, gold, _ = corpus.controls()[0]
        replies = [recorder.fixture_response(gold, True)]
        limits = dict(recorder.read(HERE / "budget.json")["per_cell_limits"], evaluator_calls=0)
        with tempfile.TemporaryDirectory() as directory:
            record = recorder.run_control(task, replies, "raw", limits, Path(directory) / "record")
            self.assertEqual(record["status"], "exhausted")
            self.assertEqual(record["effect"], "observed")
            self.assertEqual(record["stopped_before"], "score")
            self.assertEqual(record["operations"], ["initial_check", "response", "public_check", "placement"])
            with patch.object(corpus, "evaluate", side_effect=AssertionError("no observed score")):
                recorder.verify_control(task, "raw", replies, record, limits)
                for changes in ({"after": record["before"]}, {"selected": None},
                                {"usage": {**record["usage"], "strategy_tool_calls": 0}},
                                {"effect": "none"}, {"operations": []}):
                    with self.subTest(fields=list(changes)), self.assertRaises(ValueError):
                        recorder.verify_control(task, "raw", replies, dict(record, **changes), limits)

    def test_malformed_revision_keeps_both_attempts_without_scoring(self):
        """A malformed second reply ends the cell with its original observation intact."""
        task, _, _ = corpus.controls()[0]
        original = recorder.fixture_response(b"{}", False)
        replies = [original, b"false"]
        limits = recorder.read(HERE / "budget.json")["per_cell_limits"]
        with tempfile.TemporaryDirectory() as directory:
            for arm in ("revision", "lesson"):
                with self.subTest(arm=arm):
                    record = recorder.run_control(task, replies, arm, limits, Path(directory) / arm)
                    self.assertEqual(record["status"], "malformed_response")
                    self.assertEqual([item["reply_hex"] for item in record["candidates"]],
                                     [reply.hex() for reply in replies])
                    self.assertIs(record["candidates"][0]["public_pass"], False)
                    self.assertEqual(record["before"], record["after"])
                    self.assertIsNone(record["selected"])
                    self.assertEqual(record["usage"]["evaluator_calls"], 0)
                    recorder.verify_control(task, arm, replies, record, limits)

    def test_revision_refusal_preserves_first_proposal_and_all_charges(self):
        """Charge a valid refused revision and apply the original exactly once under both arms."""
        task, _, _ = corpus.controls()[0]
        limits = recorder.read(HERE / "budget.json")["per_cell_limits"]
        with tempfile.TemporaryDirectory() as directory:
            for arm in ("revision", "lesson"):
                refusal = recorder.encoded({"replacement_utf8": None, "claimed_complete": False,
                                            "refusal": "No revised proposal.", "lesson": None})
                replies = [recorder.fixture_response(b"{}", True), refusal]
                record = recorder.run_control(task, replies, arm, limits, Path(directory) / arm)
                with self.subTest(arm=arm):
                    self.assertEqual((record["selected"], record["effect"], record["status"]), (0, "observed", "rejected"))
                    self.assertEqual(record["candidates"][1]["reply_hex"], refusal.hex())
                    self.assertEqual(record["after"][task["target"]], record["candidates"][0]["sha256"])
                    self.assertEqual(record["operations"].count("placement"), 1)
                    self.assertEqual([record["usage"][k] for k in ("candidates", "strategy_tool_calls", "evaluator_calls")],
                                     [2, 3, 1])
                    recorder.verify_control(task, arm, replies, record, limits)

    def test_raw_stops_after_first_proposal_and_rejects_two_response_history(self):
        """The first raw check closes selection whether it passes or fails."""
        task, gold, _ = corpus.controls()[0]
        limits = recorder.read(HERE / "budget.json")["per_cell_limits"]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "control.json"
            for first, expected in ((b"{}", "rejected"), (gold, "verified")):
                with self.subTest(expected=expected):
                    replies = [recorder.fixture_response(first, True), recorder.fixture_response(b"{}", False)]
                    record = recorder.run_control(task, replies, "raw", limits, path)
                    self.assertEqual((record["status"], record["selected"]), (expected, 0))
                    self.assertEqual(record["operations"], ["initial_check", "response", "public_check", "placement", "score"])
                    self.assertEqual(len(record["candidates"]), 1)
                    recorder.verify_control(task, "raw", replies, record, limits)
                    sampled = recorder.run_control(task, replies, "resampling", limits, path)
                    self.assertEqual((len(sampled["candidates"]), sampled["selected"]), (2, 0))
                    with self.assertRaisesRegex(ValueError, "response after the strategy stopped"):
                        recorder.verify_control(task, "raw", replies, dict(sampled, arm="raw"), limits)

    def test_revision_arms_require_a_reply_after_a_failed_first_check(self):
        """Keep scored first passes and explicit refusals separate from absent or malformed replies."""
        task, gold, _ = corpus.controls()[0]
        limits = recorder.read(HERE / "budget.json")["per_cell_limits"]
        failed = recorder.fixture_response(b"{}", True)
        passing = recorder.fixture_response(gold, True)
        refusal = recorder.encoded({"replacement_utf8": None, "claimed_complete": False,
                                    "refusal": "No revision.", "lesson": None})
        cases = (
            ("absent first", [], "missing_proposal", None),
            ("absent second", [failed], "missing_proposal", None),
            ("first passes", [passing], "verified", 0),
            ("explicit refusal", [failed, refusal], "rejected", 0),
            ("empty second", [failed, b""], "malformed_response", None),
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "record"
            for arm in ("revision", "lesson"):
                for label, replies, status, chosen in cases:
                    with self.subTest(arm=arm, label=label):
                        record = recorder.run_control(task, replies, arm, limits, path)
                        self.assertEqual((record["status"], record["selected"]), (status, chosen))
                        self.assertEqual([c["reply_hex"] for c in record["candidates"]], [r.hex() for r in replies])
                        self.assertEqual(record["usage"]["candidates"], len(replies))
                        self.assertEqual(record["usage"]["evaluator_calls"], int(chosen is not None))
                        self.assertEqual(record["operations"].count("placement"), int(chosen is not None))
                        if chosen is None:
                            self.assertIsNone(record["evaluation"])
                            self.assertEqual(record["before"], record["after"])
                        recorder.verify_control(task, arm, replies, record, limits)
                # A raw assignment can score this failed first proposal, even without a second reply.
                invalid = recorder.run_control(task, [failed], "raw", limits, path)
                with self.assertRaisesRegex(ValueError, "Selection"):
                    recorder.verify_control(task, arm, [failed], dict(invalid, arm=arm), limits)

    def test_resampling_refusals_and_replacements_in_generation_order(self):
        """Charge both independent replies and select the earliest eligible replacement."""
        task, gold, _ = corpus.controls()[0]
        limits = recorder.read(HERE / "budget.json")["per_cell_limits"]
        examples = {"pass": recorder.fixture_response(gold, True),
                    "fail": recorder.fixture_response(b"{}", False),
                    "refuse": recorder.encoded({"replacement_utf8": None, "claimed_complete": False,
                                                "refusal": "No proposal.", "lesson": None})}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "record"
            for first in examples:
                for second in examples:
                    with self.subTest(first=first, second=second):
                        labels = (first, second)
                        replies = [examples[label] for label in labels]
                        chosen = next((i for i, label in enumerate(labels) if label == "pass"),
                                      next((i for i, label in enumerate(labels) if label == "fail"), None))
                        record = recorder.run_control(task, replies, "resampling", limits, path)
                        expected = "refused" if chosen is None else ("verified" if labels[chosen] == "pass" else "rejected")
                        self.assertEqual((record["status"], record["selected"]), (expected, chosen))
                        self.assertEqual(record["usage"]["candidates"], 2)
                        self.assertEqual([c["reply_hex"] for c in record["candidates"]], [r.hex() for r in replies])
                        self.assertTrue(all(c["parent"] is None and c["request_utf8"] == record["initial_request_utf8"]
                                            for c in record["candidates"]))
                        self.assertEqual(record["operations"].count("public_check"), 2 - labels.count("refuse"))
                        self.assertEqual(record["operations"].count("placement"), int(chosen is not None))
                        self.assertEqual(record["usage"]["evaluator_calls"], int(chosen is not None))
                        if chosen is None:
                            self.assertEqual(record["before"], record["after"])
                        recorder.verify_control(task, "resampling", replies, record, limits)
            # A third offered reply can't enter the two-response comparison.
            replies = [examples["refuse"], examples["fail"], examples["pass"]]
            record = recorder.run_control(task, replies, "resampling", limits, path)
            self.assertEqual((record["status"], record["selected"], record["usage"]["candidates"]), ("rejected", 1, 2))
            recorder.verify_control(task, "resampling", replies, record, limits)
            expanded = dict(limits, candidates=3)
            longer = recorder.run_control(task, replies, "resampling", expanded, path)
            self.assertEqual(len(longer["candidates"]), 3)
            with self.assertRaisesRegex(ValueError, "response after the strategy stopped"):
                recorder.verify_control(task, "resampling", replies, longer, limits)
            premature = recorder.run_control(task, [examples["refuse"]], "resampling", limits, path)
            with self.assertRaisesRegex(ValueError, "Terminal status"):
                recorder.verify_control(task, "resampling", [examples["refuse"], examples["pass"]], premature, limits)
            # An admitted malformed or oversized reply ends selection and retains its exact bytes.
            oversized = recorder.fixture_response(b" " * (corpus.MAX_REPLACEMENT_BYTES + 1), False)
            for replies in ([b"false", examples["pass"]], [examples["pass"], b"false"],
                            [examples["pass"], oversized]):
                record = recorder.run_control(task, replies, "resampling", limits, path)
                self.assertEqual(record["status"], "malformed_response")
                self.assertEqual(record["effect"], "none")
                self.assertEqual(record["before"], record["after"])
                received = len(record["candidates"])
                self.assertEqual([c["reply_hex"] for c in record["candidates"]], [r.hex() for r in replies[:received]])
                recorder.verify_control(task, "resampling", replies, record, limits)

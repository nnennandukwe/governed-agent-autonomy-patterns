"""Exercise the live cell with public tasks and a dispatcher that never contacts a provider."""

import copy
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import corpus
import live_cell
import recorder


class LiveCellTests(unittest.TestCase):
    """Observe requests, target effects and scorer admission at the actual cell entry point."""

    def setUp(self):
        """Stage only a public family and replace the process boundaries with local doubles."""
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.task, self.gold, self.decoy = corpus.controls()[0]
        self.study = recorder.read(recorder.HERE / "study.json")
        self.events = []
        self.requests = []
        self.workspaces = []
        self.fail = recorder.fixture_response(b"{}", True)
        self.pass_reply = recorder.fixture_response(self.gold, True)
        self.refusal = recorder.encoded({"replacement_utf8": None, "refusal": "No proposal.",
                                         "lesson": None, "claimed_complete": False})

        def prepare(private_dir, member, workspace, *, deadline):
            """Return the allowed projection and actual initial file hashes."""
            self.assertEqual(member, self.task["id"])
            projection = {key: self.task[key] for key in ("id", "target", "prompt", "initial", "checks")}
            return projection, corpus.reset(self.task, workspace)

        def request(task, arm, history, initial_check, *, deadline):
            """Render each request using a fresh copy of its own projected files."""
            with tempfile.TemporaryDirectory() as tmp:
                workspace = Path(tmp)
                corpus.reset(task, workspace)
                initial = recorder.common_request(self.study, task, workspace, initial_check)
                return recorder.request_for(self.study, initial, arm, history[0] if history else None)

        def score(private_dir, member, before, workspace, selected, *, deadline):
            """Inspect the selected file on disk independently of model claims."""
            self.assertEqual(member, self.task["id"])
            return corpus.evaluate(self.task, before, workspace, selected)

        for name, callback in (("prepare", prepare), ("candidate_request", request), ("score", score)):
            hook = patch.object(live_cell.live_isolation, name, side_effect=callback)
            hook.start()
            self.addCleanup(hook.stop)

    def run_arm(self, arm, replies, **kwargs):
        """Record every native stub call and keep this arm's private workspace available."""
        workspace = self.root / str(len(self.workspaces))
        self.workspaces.append(workspace)
        iterator = iter(replies)

        def dispatch(request, remaining_tokens, deadline):
            """Return one confirmed synthetic native receipt and charge its actual usage."""
            self.requests.append((copy.deepcopy(request), remaining_tokens))
            result = next(iterator)
            if isinstance(result, BaseException):
                raise result
            if isinstance(result, tuple):
                return result
            return result, {"input_tokens": 90, "output_tokens": 10, "list_microusd": 280,
                            "stop_reason": "end_turn", "outcome": "complete"}

        def checkpoint(kind, record):
            """Keep a snapshot at each evidence boundary before allowing the cell to continue."""
            self.events.append((kind, copy.deepcopy(record)))

        return live_cell.run_cell(self.root, self.task["id"], arm, workspace,
                                  kwargs.get("dispatch", dispatch), kwargs.get("checkpoint", checkpoint))

    def test_first_requests_and_independent_resampling(self):
        """Different arms share the same first request, and earlier checks never reach a resample."""
        first = []
        for arm in recorder.ARMS:
            entries = [self.pass_reply]
            if arm == "resampling":
                entries = [recorder.fixture_response(self.decoy, True), self.pass_reply]
            start = len(self.requests)
            row = self.run_arm(arm, entries)
            first.append(self.requests[start][0])
            self.assertEqual(row["selected"], 0)
            self.assertEqual(row["evaluator_calls"], 1)
            self.assertIs(row["mandatory_failure"], None if arm == "resampling" else False)
            self.assertTrue(row["claimed_complete"])
            if arm == "resampling":
                self.assertEqual(row["status"], "rejected")
                self.assertEqual(self.requests[start][0], self.requests[start + 1][0])
                self.assertEqual([n for _, n in self.requests[start:]], [12000, 11900])
                self.assertEqual(row["input_tokens"], 180)
                self.assertEqual(row["calls"], 2)
            else:
                self.assertEqual(row["status"], "verified")
        self.assertTrue(all(request == first[0] for request in first))

    def test_revision_lesson_and_refusal(self):
        """Use only the checked first attempt's failure to choose and attribute one revision."""
        for arm in ("revision", "lesson"):
            start = len(self.requests)
            revised = recorder.fixture_response(self.gold, True, "Restore the missing setting." if arm == "lesson" else None)
            row = self.run_arm(arm, [self.fail, revised])
            self.assertEqual((row["status"], row["selected"], row["calls"], row["tool_calls"]), ("verified", 1, 2, 4))
            self.assertEqual(self.requests[start + 1][0]["previous_reply_hex"], self.fail.hex())
            self.assertIs(self.requests[start + 1][0]["previous_public_pass"], False)
            if arm == "lesson":
                self.assertEqual(row["lesson"]["sha256"], row["candidates"][0]["sha256"])
                self.assertEqual(row["lesson"]["member"], self.task["id"])
            refused = self.run_arm(arm, [self.fail, self.refusal])
            self.assertEqual((refused["status"], refused["selected"]), ("rejected", 0))
            self.assertEqual((refused["tool_calls"], refused["calls"]), (3, 2))
            for missing, expected in ((b"", "missing_proposal"), (b"bad", "malformed_response")):
                stopped = self.run_arm(arm, [self.fail, missing])
                self.assertEqual(stopped["status"], expected)
                self.assertEqual((stopped["selected"], stopped["effect"], stopped["evaluator_calls"]), (None, "none", 0))
                self.assertEqual(stopped["after"], stopped["initial"])

    def test_native_stops_and_budget(self):
        """Known native refusals consume calls and an incomplete message can't earn score credit."""
        usage = {"input_tokens": 90, "output_tokens": 10, "list_microusd": 280,
                 "outcome": "refused", "stop_reason": "refusal"}
        native_refusal = (b"No output.", usage)
        row = self.run_arm("resampling", [native_refusal, self.pass_reply])
        self.assertEqual((row["status"], row["selected"], row["calls"]), ("verified", 1, 2))
        stopped = self.run_arm("resampling", [native_refusal, native_refusal])
        self.assertEqual((stopped["status"], stopped["evaluator_calls"]), ("refused", 0))
        incomplete = self.run_arm("raw", [(self.pass_reply, dict(usage, outcome="incomplete", stop_reason="max_tokens"))])
        self.assertEqual((incomplete["status"], incomplete["effect"], incomplete["calls"]), ("incomplete", "none", 1))
        full = dict(usage, input_tokens=10000, output_tokens=2000, list_microusd=40000, outcome="complete", stop_reason="end_turn")
        exhausted = self.run_arm("revision", [(self.fail, full)])
        self.assertEqual((exhausted["status"], exhausted["reason"], exhausted["calls"]), ("exhausted", "total_tokens", 1))
        with self.assertRaisesRegex(ValueError, "exceeds the remaining token"):
            self.run_arm("raw", [(self.pass_reply, dict(full, input_tokens=10001))])

    def test_uncertain_provider_or_target_never_repeats(self):
        """An ambiguous send or a failed post-rename observation propagates its original error."""
        failure = TimeoutError("delivery unknown")
        with self.assertRaises(TimeoutError) as caught:
            self.run_arm("resampling", [failure, self.pass_reply])
        self.assertIs(caught.exception, failure)
        self.assertEqual(len(self.requests), 1)
        self.assertTrue(self.workspaces[-1].joinpath(corpus.TARGET).is_file())
        self.assertFalse(any(kind == "selection" for kind, _ in self.events))
        replace = live_cell.os.replace

        def lose_reply(source, target):
            """Install the real target and then report an I/O error to its caller."""
            replace(source, target)
            raise OSError("post-replacement observation failed")

        with patch.object(live_cell.os, "replace", side_effect=lose_reply):
            with self.assertRaisesRegex(OSError, "post-replacement"):
                self.run_arm("raw", [self.pass_reply])
        self.assertEqual(self.workspaces[-1].joinpath(corpus.TARGET).read_bytes(), self.gold)
        self.assertEqual(self.events[-1][0], "selection")
        self.assertEqual(self.events[-1][1]["effect"], "uncertain")
        self.assertEqual(self.events[-1][1]["evaluator_calls"], 0)

    def test_absolute_deadline_and_score_binding(self):
        """A deadline can expire before the first call, or after a real verdict but before credit."""
        row = live_cell.run_cell(self.root, self.task["id"], "raw", self.root / "unused",
                                 self.fail, lambda *args: None, deadline=0)
        self.assertEqual((row["status"], row["calls"], row["evaluator_calls"]), ("timeout", 0, 0))
        actual_score = live_cell.live_isolation.score.side_effect
        now = [100]

        def late(*args, **kwargs):
            """Return the actual verdict after the monotonic cutoff."""
            result = actual_score(*args, **kwargs)
            now[0] += 301
            return result

        with patch.object(live_cell.time, "monotonic", side_effect=lambda: now[0]), \
             patch.object(live_cell.time, "monotonic_ns", side_effect=lambda: now[0] * 1_000_000_000), \
             patch.object(live_cell.live_isolation, "score", side_effect=late):
            timed = self.run_arm("raw", [self.pass_reply])
        self.assertEqual((timed["status"], timed["evaluator_calls"]), ("timeout", 1))
        self.assertTrue(timed["evaluation"]["correct"])

        def unrelated(*args, **kwargs):
            """Replace the genuine selected subject digest with another identity."""
            return dict(actual_score(*args, **kwargs), selected_sha256="0" * 64)

        with patch.object(live_cell.live_isolation, "score", side_effect=unrelated):
            with self.assertRaisesRegex(ValueError, "bound to the observed selection"):
                self.run_arm("raw", [self.pass_reply])

    def test_mandatory_settings_need_independent_or_named_evidence(self):
        """A decoy's public pass can't prove preservation of every mandatory setting."""
        decoy = self.run_arm("raw", [recorder.fixture_response(self.decoy, True)])
        self.assertIsNone(decoy["mandatory_failure"])
        self.assertEqual(decoy["mandatory_evidence"]["reason"], "incorrect_configuration_without_complete_preservation_evidence")
        erased = self.run_arm("raw", [self.fail])
        self.assertIs(erased["mandatory_failure"], True)
        self.assertEqual(erased["mandatory_evidence"]["path"], self.task["checks"][0]["path"])
        malformed = self.run_arm("raw", [recorder.fixture_response(b"not JSON", True)])
        self.assertEqual((malformed["status"], malformed["claimed_complete"]), ("rejected", True))
        self.assertIs(malformed["mandatory_failure"], True)
        original = recorder.fixture_response(recorder.encoded(self.task["initial"]), False)
        no_change = self.run_arm("raw", [original])
        self.assertEqual(no_change["status"], "rejected")
        self.assertIs(no_change["mandatory_failure"], False)
        self.assertEqual(no_change["mandatory_evidence"]["reason"], "initial_settings_semantically_unchanged")
        gold = self.run_arm("raw", [self.pass_reply])
        self.assertEqual(gold["mandatory_evidence"]["reason"], "independent_expected_state_matched")

    def test_scorer_cant_credit_a_failed_public_requirement(self):
        """A self-claimed completion and impossible positive verdict put the cell on hold."""
        actual_score = live_cell.live_isolation.score.side_effect

        def impossible(*args, **kwargs):
            """Retain authentic hashes while substituting a verdict the public constraint rules out."""
            return dict(actual_score(*args, **kwargs), correct=True, effect_valid=True, reasons=[])

        with patch.object(live_cell.live_isolation, "score", side_effect=impossible):
            with self.assertRaisesRegex(ValueError, "credited an invalid effect"):
                self.run_arm("raw", [self.fail])
        self.assertEqual(self.events[-1][0], "evaluation")
        self.assertIsNone(self.events[-1][1]["mandatory_failure"])

"""Exercise the offline evaluator and the boundaries of the frozen split."""

import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest

import corpus


class EvaluatorTests(unittest.TestCase):
    """Read each outcome from a fresh directory that contains only public task inputs."""

    def setUp(self):
        """Create an empty parent so each case can reset its own workspace."""
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.parent = Path(self.scratch.name)

    def test_two_gold_controls_and_false_positives(self):
        """A selected public pass can still lose a value required by the full task."""
        for task, gold, decoy in corpus.controls():
            for label, selected in (("gold", gold), ("decoy", decoy), ("absent", None)):
                with self.subTest(member=task["id"], label=label):
                    workspace = self.parent / (task["id"] + label)
                    before = corpus.reset(task, workspace)
                    self.assertEqual(before, corpus.snapshot(workspace))
                    if selected is not None:
                        self.assertTrue(corpus.public_checks(task, selected))
                        (workspace / task["target"]).write_bytes(selected)
                    outcome = corpus.evaluate(task, before, workspace, selected)
                    self.assertEqual(outcome["correct"], label == "gold")
                    self.assertEqual(outcome["effect_valid"], selected is not None)
                    self.assertFalse(corpus.public_checks(task, b'{}'))

    def test_selection_protected_inputs_and_reset_are_independent(self):
        """Changing a protected file, the start or selected bytes defeats credit."""
        task, gold, decoy = corpus.controls()[0]
        workspace = self.parent / "work"
        before = corpus.reset(task, workspace)
        initial_bytes = (workspace / task["target"]).read_bytes()
        (workspace / task["target"]).write_bytes(gold)
        self.assertFalse(corpus.evaluate(task, before, workspace, decoy)["effect_valid"])
        self.assertFalse(corpus.evaluate(task, {}, workspace, gold)["effect_valid"])
        (workspace / "TASK.txt").write_text("caller supplied a passing verdict")
        self.assertFalse(corpus.evaluate(task, before, workspace, gold)["effect_valid"])
        with self.assertRaises(ValueError):
            corpus.reset(task, workspace)
        clean = self.parent / "clean"
        self.assertEqual(corpus.reset(task, clean), before)
        self.assertEqual((clean / task["target"]).read_bytes(), initial_bytes)

    def test_unsafe_targets_and_extra_entries_never_receive_credit(self):
        """An alias or an additional workspace file defeats the one-target policy."""
        task, gold, _ = corpus.controls()[0]
        outside = self.parent / "outside"
        outside.write_bytes(gold)
        for kind in ("symlink", "hardlink", "extra"):
            with self.subTest(kind=kind):
                workspace = self.parent / kind
                before = corpus.reset(task, workspace)
                target = workspace / task["target"]
                if kind == "extra":
                    target.write_bytes(gold)
                    (workspace / "another").write_bytes(gold)
                else:
                    target.unlink()
                    if kind == "symlink":
                        target.symlink_to(outside)
                    else:
                        os.link(outside, target)
                outcome = corpus.evaluate(task, before, workspace, gold)
                self.assertFalse(outcome["correct"])
                self.assertFalse(outcome["effect_valid"])
                self.assertEqual(outside.read_bytes(), gold)

    def test_duplicate_members_types_and_proposal_limits(self):
        """Public checks reject ambiguous objects, oversized proposals and wrong types."""
        task, gold, _ = corpus.controls()[0]
        self.assertFalse(corpus.public_checks(task, b'{"duplicate":1,"duplicate":2}'))
        self.assertFalse(corpus.public_checks(task, gold + b" " * corpus.MAX_REPLACEMENT_BYTES))
        probe = {"checks": [{"path": ["retries"], "equals": 1}]}
        self.assertFalse(corpus.public_checks(probe, b'{"retries":true}'))
        self.assertTrue(corpus.public_checks(probe, b'{"retries":1}'))
        prefix = {"checks": [{"path": ["row"], "starts_with": "first,"}]}
        self.assertTrue(corpus.public_checks(prefix, b'{"row":"first,second"}'))
        self.assertFalse(corpus.public_checks(prefix, b'{"row":1}'))

    def test_candidate_files_exclude_evaluator_and_host_inputs(self):
        """The candidate gets only instructions, its starting object and public assertions."""
        for task, _, _ in corpus.controls():
            workspace = self.parent / task["id"]
            corpus.reset(task, workspace)
            self.assertEqual(set(path.name for path in workspace.iterdir()),
                             {"TASK.txt", "checks.json", "settings.json"})
            self.assertEqual(json.loads((workspace / task["target"]).read_bytes()), task["initial"])
            self.assertEqual(json.loads((workspace / "checks.json").read_bytes()), task["checks"])
            self.assertEqual((workspace / "TASK.txt").read_text(), task["prompt"] + "\n")


class SplitTests(unittest.TestCase):
    """Verify public membership and private commitments without reading real held-out cases."""

    def test_public_index(self):
        """Publish only IDs and commitments for the two validation and eight held-out members."""
        verified = corpus.verify()
        self.assertEqual(verified["public_verified"], 2)
        self.assertEqual(verified["private_verified"], 0)
        labels = [split for split, _ in corpus.members()]
        self.assertEqual(labels, ["development"] * 2 + ["validation"] * 2 + ["held_out"] * 8)
        self.assertEqual(len(set(member for _, member in corpus.members())), 12)

    def test_commitment_and_private_file_custody(self):
        """Private verification rejects a substituted member, stale digest or exposed file."""
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            directory.chmod(0o700)
            task, _, _ = corpus.controls()[0]
            payload = dict(task, id="1" * 32, salt="2" * 64)
            payload.pop("decoy")
            entry = {"id": payload["id"], "split": "validation"}
            raw = json.dumps(payload, ensure_ascii=False).encode()
            entry["sha256"] = hashlib.sha256(raw).hexdigest()
            path = directory / (entry["id"] + ".json")
            path.write_bytes(raw)
            path.chmod(0o600)
            self.assertEqual(corpus.read_member(directory, entry)["id"], payload["id"])
            path.chmod(0o644)
            with self.assertRaises(ValueError):
                corpus.read_member(directory, entry)
            path.chmod(0o600)
            path.write_bytes(raw + b" ")
            with self.assertRaises(ValueError):
                corpus.read_member(directory, entry)
            entry["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
            wrong = dict(entry, id="3" * 32)
            path.rename(directory / (wrong["id"] + ".json"))
            with self.assertRaises(ValueError):
                corpus.read_member(directory, wrong)

    def test_rendered_check_bytes_match_the_workspace_limit(self):
        """Admit the exact UTF-8 byte limit and reject one extra byte before reset."""
        # non-English test data: this prefix takes two UTF-8 bytes.
        multibyte_prefix = "\u00e9"
        value = multibyte_prefix + "x" * 35000
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            member_id = "0" * 32
            path = directory / (member_id + ".json")
            for excess in (0, 1):
                with self.subTest(excess=excess):
                    limit = corpus.MAX_REPLACEMENT_BYTES + excess
                    checks = [{"path": ["field"], "equals": value},
                              {"path": ["field"], "starts_with": multibyte_prefix}]
                    checks[1]["starts_with"] += "x" * (limit - len(corpus.encode(checks)))
                    member = {"schema": "feedback-repair-task/1", "id": member_id, "salt": "1" * 64,
                              "family": "size-limit", "prompt": "Restore the catalog value.",
                              "target": "settings.json", "initial": {"field": "old"},
                              "expected": {"field": value}, "checks": checks}
                    raw = corpus.encode(member)
                    path.write_bytes(raw)
                    path.chmod(0o600)
                    entry = {"id": member_id, "split": "validation", "sha256": corpus.digest(raw)}
                    workspace = directory / str(limit)
                    self.assertEqual(len(corpus.encode(checks)), limit)
                    self.assertLess(len(raw), corpus.MAX_BUNDLE_BYTES)
                    if excess:
                        with self.assertRaisesRegex(ValueError, "initial workspace file"):
                            corpus.read_member(directory, entry)
                        with self.assertRaisesRegex(ValueError, "initial workspace file"):
                            corpus.reset(member, workspace)
                        self.assertFalse(workspace.exists())
                    else:
                        admitted = corpus.read_member(directory, entry)
                        corpus.reset(admitted, workspace)
                        self.assertEqual((workspace / "checks.json").stat().st_size, limit)

    def test_duplicate_reordered_or_extra_split_fields(self):
        """Duplicate families, swapped assignments and exposed private fields invalidate the index."""
        for change in ("duplicate_id", "duplicate_digest", "swap", "answer"):
            with self.subTest(change=change):
                index = json.loads((corpus.ROOT / "split.json").read_bytes())
                entries = index["members"]
                if change == "duplicate_id":
                    entries[-1]["id"] = entries[0]["id"]
                elif change == "duplicate_digest":
                    entries[-1]["sha256"] = entries[0]["sha256"]
                elif change == "swap":
                    entries[0], entries[-1] = entries[-1], entries[0]
                else:
                    entries[-1]["answer"] = "leaked"
                with self.assertRaises(ValueError):
                    corpus._index_entries(index)

    def test_missing_bundle_prevents_private_verification(self):
        """An empty private directory can't silently fall back to public fixtures."""
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            directory.chmod(0o700)
            with self.assertRaisesRegex(ValueError, "missing or unexpected"):
                corpus.verify(directory)
            missing = directory / "missing"
            with self.assertRaises(FileNotFoundError):
                corpus.verify(missing)
            directory.chmod(0o755)
            with self.assertRaisesRegex(ValueError, "owner custody"):
                corpus.verify(directory)


if __name__ == "__main__":
    unittest.main()

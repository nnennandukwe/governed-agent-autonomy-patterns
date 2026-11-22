"""Qualify OS-enforced study boundaries with public cases and disposable private inputs."""

import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import corpus
import live_isolation
import recorder


class HostAdmissionTests(unittest.TestCase):
    """An unsupported operating system must stop before a study worker can run."""

    def test_unsupported_host_refuses_worker(self):
        """Every role checks its execution host before invoking a subprocess."""
        with patch.object(sys, "platform", "linux"), patch("subprocess.run") as launch:
            for role in ("prepare", "request", "score", "probe"):
                with self.subTest(role=role), self.assertRaisesRegex(RuntimeError, "macOS"):
                    live_isolation._run(role, {}, [])
            launch.assert_not_called()


@unittest.skipUnless(sys.platform == "darwin", "requires the deployed macOS kernel sandbox")
class IsolationTests(unittest.TestCase):
    """Run the production process boundary on the host used for live collection."""

    def test_kernel_denials_and_exact_public_request(self):
        """Read the actual canary results and compare the request with the original public control."""
        with patch.dict(os.environ, {"GAAP_SYNTHETIC_CREDENTIAL": "test value"}):
            proof = live_isolation.qualify()
        self.assertEqual(set(proof["denials"]), {"private_read", "other_cell_read", "credential_read",
                                                 "scorer_read", "protected_write", "network"})
        self.assertTrue(all(value in (1, 13) for value in proof["denials"].values()))
        self.assertFalse(proof["inherited_canary_fd"])
        self.assertEqual(proof["environment_names"], [])
        self.assertEqual(len(proof["public_request_sha256"]), 64)

    def test_committed_private_projection_and_separate_scoring(self):
        """Only public task fields cross custody. The scorer reads the committed original afresh."""
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            private = root / "private"
            private.mkdir(mode=0o700)
            source = corpus.controls()[0][0]
            member = "7" * 32
            hidden = "host-only scoring canary"
            expected = dict(source["expected"], audit=hidden)
            task = dict(source, id=member, salt="8" * 64, expected=expected)
            del task["decoy"]
            original = corpus.encode(task)
            path = private / (member + ".json")
            path.write_bytes(original)
            path.chmod(0o600)
            entry = {"split": "held_out", "id": member, "sha256": corpus.digest(original)}
            workspace = root / "selected"
            with patch.object(corpus, "_index", return_value=[entry]):
                projected, before = live_isolation.prepare(private, member, workspace)
                self.assertEqual(set(projected), {"id", "target", "prompt", "initial", "checks"})
                first = live_isolation.candidate_request(projected, "raw", [], False)
                for secret in (hidden, task["salt"], member, task["family"], str(private)):
                    self.assertNotIn(secret, json.dumps(first))
                passing = corpus.encode(expected)
                (workspace / corpus.TARGET).write_bytes(passing)
                result = live_isolation.score(private, member, before, workspace, passing)
                self.assertTrue(result["correct"])
                self.assertEqual(result, corpus.evaluate(task, before, workspace, passing))
                self.assertFalse(live_isolation.score(private, member, before, workspace, b"{}")["correct"])
                path.write_bytes(original + b" ")
                with self.assertRaisesRegex(RuntimeError, "isolated score"):
                    live_isolation.score(private, member, before, workspace, passing)
                with self.assertRaisesRegex(RuntimeError, "isolated prepare"):
                    live_isolation.prepare(private, member, root / "second")

    def test_fresh_attempts_matches_original_arm_inputs(self):
        """Resamples see a fresh common request. Revisions get only the failed reply and its check."""
        example = corpus.controls()[0][0]
        task = {key: example[key] for key in live_isolation.PUBLIC_FIELDS}
        failed = {"reply_hex": recorder.fixture_response(b"{}", True).hex(), "public_pass": False}
        study = json.loads((live_isolation.HERE / "study.json").read_bytes())
        with tempfile.TemporaryDirectory() as name:
            workspace = Path(name)
            corpus.reset(task, workspace)
            initial = recorder.common_request(study, task, workspace, False)
        for arm in recorder.ARMS:
            with self.subTest(arm=arm):
                self.assertEqual(live_isolation.candidate_request(task, arm, [], False), initial)
                if arm in ("revision", "lesson"):
                    self.assertEqual(live_isolation.candidate_request(task, arm, [failed], False),
                                     recorder.request_for(study, initial, arm, failed))
                else:
                    with self.assertRaises(ValueError):
                        live_isolation.candidate_request(task, arm, [failed], False)
        with self.assertRaises(ValueError):
            live_isolation.candidate_request(example, "raw", [], False)
        with self.assertRaises(ValueError):
            live_isolation.candidate_request(task, "revision", [dict(failed, other_cell=True)], False)

    def test_expired_deadline_stops_before_reset(self):
        """The parent receives a timeout before the custodian can return its projection."""
        with tempfile.TemporaryDirectory() as name:
            workspace = Path(name) / "selected"
            member = corpus.members()[0][1]
            with self.assertRaises(TimeoutError):
                live_isolation.prepare(None, member, workspace, deadline=time.monotonic() - 1)
            self.assertFalse(workspace.exists())


if __name__ == "__main__":
    unittest.main()

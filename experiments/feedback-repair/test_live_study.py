"""Exercise live enrollment, dispatch and recovery with synthetic members and a local provider."""

from contextlib import ExitStack, contextmanager
from decimal import Decimal
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import corpus
import live_cell
import live_isolation
import live_provider
import live_study as study
import recorder
from test_live_provider import ENV, HEADERS, native_reply


def synthetic_members(root):
    """Commit ten disposable bundles alongside the two real public development controls."""
    private = root / "private"
    private.mkdir(mode=0o700)
    index = corpus._index()
    public = corpus.controls()[0][0]
    for ordinal, entry in enumerate(index[2:]):
        family = "synthetic-" + chr(ord("a") + ordinal)
        task = dict(public, id=entry["id"], family=family, salt=f"{ordinal:064x}",
                    prompt=public["prompt"] + " " + family)
        del task["decoy"]
        raw = corpus.encode(task)
        path = private / (entry["id"] + ".json")
        path.write_bytes(raw)
        path.chmod(0o600)
        entry["sha256"] = corpus.digest(raw)
    (root / "test-index.json").write_bytes(corpus.encode(index))
    return private


@contextmanager
def controlled_host(root, endpoint=None):
    """Replace service transport and OS isolation only, keeping the real ledger and corpus checks."""
    index = json.loads((root / "test-index.json").read_bytes())
    study_spec = recorder.read(recorder.HERE / "study.json")
    gold_by_initial = {corpus.encode(task["initial"]).decode(): gold for task, gold, _ in corpus.controls()}
    calls = []

    def original(private_dir, member):
        """Load exactly one public or synthetic member by its frozen byte commitment."""
        entry, directory = live_isolation._member(private_dir, member)
        return corpus.read_member(directory, entry)

    def prepare(private_dir, member, workspace, deadline=None):
        """Reset a new candidate workspace from its allowlisted projection."""
        task = original(private_dir, member)
        projected = {key: task[key] for key in live_isolation.PUBLIC_FIELDS}
        return projected, corpus.reset(projected, workspace)

    def request(task, arm, history, initial_check, deadline=None):
        """Exercise the original request functions for every enrolled first input."""
        with tempfile.TemporaryDirectory() as name:
            workspace = Path(name)
            corpus.reset(task, workspace)
            first = recorder.common_request(study_spec, task, workspace, initial_check)
            return recorder.request_for(study_spec, first, arm, history[0] if history else None)

    def score(private_dir, member, before, workspace, selected, deadline=None):
        """Score the selected bytes using the committed synthetic original."""
        return corpus.evaluate(original(private_dir, member), before, workspace, selected)

    def transport(method, path, wire, deadline, **options):
        """Answer free counts and record the paid request outside the dispatcher's ledger."""
        if method != "POST":
            raise AssertionError("This synthetic broker only accepts POST.")
        if path == "/v1/messages/count_tokens":
            return 200, HEADERS, b'{"input_tokens":20}'
        if path != "/v1/messages":
            raise AssertionError("Unrecognized synthetic endpoint.")
        if endpoint is not None:
            with socket.create_connection(("127.0.0.1", endpoint), timeout=10) as connection:
                connection.sendall(len(wire).to_bytes(4, "big") + wire)
                if not connection.recv(1):
                    raise RuntimeError("The external observer withheld its response.")
        calls.append(wire)
        payload = native_reply()
        payload["id"] = f"msg_fake{len(calls)}"
        request_body = json.loads(json.loads(wire)["messages"][0]["content"])
        gold = gold_by_initial[request_body["files"][corpus.TARGET]]
        payload["content"] = [{"type": "text", "text": recorder.fixture_response(gold, True).decode()}]
        return 200, dict(HEADERS, **{"request-id": f"req_call{len(calls)}"}), corpus.encode(payload)

    with ExitStack() as stack:
        for target, field, value in (
                (corpus, "_index", lambda: index), (study, "identity", lambda: {"synthetic_image": True}),
                (live_isolation, "qualify", lambda: {"synthetic_boundary": True}),
                (live_isolation, "prepare", prepare), (live_isolation, "score", score),
                (live_isolation, "candidate_request", request), (live_provider, "_request", transport)):
            stack.enter_context(patch.object(target, field, value))
        stack.enter_context(patch.dict(os.environ, ENV, clear=True))
        yield calls


def run_child(root, expected, port):
    """Enter the real collector in a fresh process using only the synthetic loopback transport."""
    private = root / "private"
    home = study.private_home(private)
    with controlled_host(root, port), study.ledger(home) as db:
        frozen = study.frozen_record(db, expected)
        try:
            study.collect(db, private, home, frozen)
        except study.Hold as error:
            print(str(error), flush=True)
            return
        raise AssertionError("The synthetic collector unexpectedly completed.")


def admin_capture(interval, requests):
    """Build native admin pages from the fake service's independently counted deliveries."""
    workspace, key, _, _ = interval
    group = {"workspace_id": workspace, "api_key_id": key, "model": "claude-sonnet-5",
             "service_tier": "standard", "inference_geo": "global"}
    amount = format(Decimal(requests) * Decimal("0.009"), ".6f")
    metrics = {"usage": {"uncached_input_tokens": 20 * requests, "output_tokens": 5 * requests,
                         "total_tokens": 25 * requests, "requests": requests, "cache_read_input_tokens": 0,
                         "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 0},
                         "server_tool_use": {"web_search_requests": 0}},
               "cost": {"list_amount": amount, "amount": amount, "currency": "USD", "requests": requests}}
    responses = []
    for kind, values in metrics.items():
        row = {"group": group, "starting_at": "2026-11-20T09:10:00Z", "ending_at": "2026-11-20T09:11:00Z",
               kind: values}
        page = {"data": [row], "has_more": False, "next_page": None,
                "data_refreshed_at": "2026-11-21T04:00:00Z"}
        responses.append((200, {"request-id": "req_admin_" + kind}, corpus.encode(page)))
    with patch.object(live_provider, "_request", side_effect=responses):
        return live_provider.capture_billing(*interval, time.monotonic() + 5)


class StudyTests(unittest.TestCase):
    """Observe the enrolled database across separate processes and caller-held freeze digests."""

    def setUp(self):
        """Create a fresh owner-only study with synthetic private commitments."""
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name).resolve()
        self.private = synthetic_members(self.root)
        self.home = study.private_home(self.private)
        boundary = controlled_host(self.root)
        self.calls = boundary.__enter__()
        self.addCleanup(boundary.__exit__, None, None, None)
        self.expected = study.freeze_run(self.private)

    def activate(self, db, cell):
        """Set up the same running assignment and native scope required by the direct dispatch API."""
        with db:
            db.execute("INSERT OR IGNORE INTO metadata VALUES ('billing_scope',?)",
                       (recorder.encoded(list(live_provider.read_account_scope())),))
            db.execute("UPDATE cells SET state='running' WHERE id=?", (cell,))

    def test_freeze_denominator_request_identity_and_tampering(self):
        """Every arm starts from the same committed input and each assignment starts unattempted."""
        with study.ledger(self.home) as db:
            frozen = study.frozen_record(db, self.expected)
            pairs = [[split, member, arm] for split, member in corpus.members() for arm in recorder.ARMS]
            self.assertEqual(frozen["assignments"], pairs)
            self.assertEqual(len(frozen["initial_requests"]), 12)
            self.assertEqual(len(set(frozen["initial_requests"].values())), 12)
            self.assertEqual(db.execute("SELECT count(*) FROM cells WHERE state='unattempted'").fetchone()[0], 48)
            with self.assertRaisesRegex(study.Hold, "digest"):
                study.frozen_record(db, "0" * 64)
            self.activate(db, 1)
            with self.assertRaisesRegex(study.Hold, "first public input"):
                study.dispatch(db, 1, {"substituted": True}, 12000, time.monotonic() + 5,
                               frozen["initial_requests"][pairs[0][1]])
            self.assertEqual(study.charges(db)["requests"], 0)
            with db:
                db.execute("DELETE FROM cells WHERE id=48")
            with self.assertRaisesRegex(study.Hold, "48 frozen"):
                study.frozen_record(db, self.expected)
            self.assertEqual(self.calls, [])

    def test_receipt_and_cap_at_paid_boundary(self):
        """A settled call retains its exact wire, native response and charges across a cap change."""
        with study.ledger(self.home) as db:
            member = corpus.members()[0][1]
            projected, _ = live_isolation.prepare(self.private, member, self.root / "first")
            request = live_isolation.candidate_request(projected, "raw", [], False)
            frozen = study.frozen_record(db, self.expected)
            expected = frozen["initial_requests"][member]
            self.activate(db, 1)
            with patch.object(study, "CAP", 1), self.assertRaises(live_cell.CellLimit):
                study.dispatch(db, 1, request, 12000, time.monotonic() + 5, expected)
            self.assertEqual(self.calls, [])
            _, usage = study.dispatch(db, 1, request, 12000, time.monotonic() + 5, expected)
            self.assertEqual(usage["list_microusd"], 90)
            row = db.execute("SELECT * FROM calls").fetchone()
            self.assertEqual((row["wire"], row["state"], row["request_id"]),
                             (self.calls[0], "settled", "req_call1"))
            self.assertEqual(json.loads(row["body"])["usage"]["input_tokens"], 20)
            self.assertEqual(json.loads(row["usage"])["output_tokens"], 5)
            with patch.object(study, "CAP", 90), self.assertRaises(live_cell.CellLimit):
                study.dispatch(db, 1, request, 11975, time.monotonic() + 5, expected)
            self.assertEqual(len(self.calls), 1)
            self.assertEqual(study.charges(db), {"requests": 1, "input_tokens": 20,
                                                "output_tokens": 5, "list_microusd": 90})

    def test_empty_and_complete_reports_require_original_native_billing(self):
        """Keep the empty denominator, then match every stored call to separate scoped admin evidence."""
        with study.ledger(self.home) as db:
            frozen = study.frozen_record(db, self.expected)
            empty = study.report(db, frozen)
            self.assertEqual(empty["states"], {"unattempted": 48, "finished": 0, "running": 0})
            self.assertEqual(len(empty["assignments"]), 48)
            self.assertIsNone(empty["outcomes"]["held_out"]["raw"]["verified_per_attempt"])
            self.assertIsNone(empty["billing"])
            self.assertEqual(empty["decision"]["state"], "unresolved")
            with self.assertRaisesRegex(study.Hold, "all 48 dispositions"):
                study.reconcile(db)
            with patch.object(study, "utc", return_value="2026-11-20T09:10:30+00:00"):
                study.collect(db, self.private, self.home, frozen)
            delivered = len(self.calls)
            self.assertEqual(delivered, 60)
            settled = study.report(db, frozen)
            self.assertEqual(settled["states"], {"unattempted": 0, "finished": 48, "running": 0})
            self.assertEqual(settled["settled_usage"]["requests"], delivered)
            self.assertEqual(settled["settled_usage"]["list_microusd"], delivered * 90)
            first_requests = db.execute("""SELECT member, wire FROM calls JOIN cells ON cell=cells.id
                                           WHERE ordinal=1 ORDER BY cells.id""").fetchall()
            self.assertEqual(len(first_requests), 48)
            by_family = {}
            for member, wire in first_requests:
                public = json.loads(json.loads(wire)["messages"][0]["content"])
                self.assertEqual(corpus.digest(recorder.encoded(public)), frozen["initial_requests"][member])
                by_family.setdefault(member, []).append(wire)
            self.assertEqual(len(by_family), 12)
            self.assertTrue(all(len(wires) == 4 and len(set(wires)) == 1 for wires in by_family.values()))
            self.assertIsNone(settled["billing"])
            self.assertEqual(settled["decision"]["state"], "unresolved")
            interval = study.billing_interval(db)
            capture = admin_capture(interval, delivered)
            with self.assertRaises(live_provider.ProviderError):
                study.reconcile(db, capture, "0" * 64)
            missing_call = admin_capture(interval, delivered - 1)
            with self.assertRaises(live_provider.ProviderError):
                study.reconcile(db, missing_call, corpus.digest(missing_call))
            self.assertIsNone(study.report(db, frozen)["billing"])
            study.reconcile(db, capture, corpus.digest(capture))
            final = study.report(db, frozen)
            self.assertEqual(final["billing"]["billed_microusd"], delivered * 90)
            self.assertEqual(final["billing"]["capture_sha256"], corpus.digest(capture))
            self.assertNotIn("capture", final["billing"])
            self.assertEqual(final["settled_usage"], final["maximum_committed_usage"])
            study.collect(db, self.private, self.home, frozen)
            self.assertEqual(len(self.calls), delivered)

    def test_kill_after_external_delivery_preserves_reservation_and_prevents_replay(self):
        """Withhold the external response, kill the lock owner, and retry from a fresh process."""
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen(1)
            listener.settimeout(8)
            command = [sys.executable, "-B", str(Path(__file__).resolve()), "--collector",
                       str(self.root), self.expected, str(listener.getsockname()[1])]
            with subprocess.Popen(command, env={}, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as child:
                try:
                    connection, _ = listener.accept()
                    with connection, connection.makefile("rb") as stream:
                        connection.settimeout(3)
                        size = int.from_bytes(stream.read(4), "big")
                        self.assertTrue(0 < size < 200_000)
                        received = stream.read(size)
                        self.assertEqual(len(received), size)
                        with self.assertRaisesRegex(study.Hold, "Another process"), study.ledger(self.home):
                            self.fail("Another writer entered the live collector.")
                        child.kill()
                        child.communicate(timeout=5)
                        self.assertEqual(child.returncode, -signal.SIGKILL)
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.communicate(timeout=5)
            resumed = subprocess.run(command, env={}, capture_output=True, timeout=8)
            self.assertEqual(resumed.returncode, 0, resumed.stderr)
            self.assertIn(b"interrupted cell", resumed.stdout)
            listener.settimeout(0.15)
            with self.assertRaises(TimeoutError):
                listener.accept()
        with study.ledger(self.home) as db:
            frozen = study.frozen_record(db, self.expected)
            rows = db.execute("SELECT * FROM calls").fetchall()
            self.assertEqual(len(rows), 1)
            self.assertEqual((rows[0]["state"], rows[0]["wire"], rows[0]["body"], rows[0]["usage"]),
                             ("reserved", received, None, None))
            self.assertGreater(study.charges(db)["list_microusd"], 0)
            self.assertEqual(db.execute("SELECT count(*) FROM cells WHERE state='running'").fetchone()[0], 1)
            self.assertEqual(db.execute("SELECT count(*) FROM cells WHERE state='unattempted'").fetchone()[0], 47)
            self.assertEqual(len(frozen["assignments"]), 48)
            partial = study.report(db, frozen)
            self.assertEqual(partial["states"], {"unattempted": 47, "running": 1, "finished": 0})
            self.assertEqual(partial["settled_usage"]["requests"], 0)
            self.assertEqual(partial["maximum_committed_usage"]["requests"], 1)
            self.assertIsNone(partial["billing"])
            self.assertEqual(partial["decision"]["state"], "unresolved")


if __name__ == "__main__":
    if len(sys.argv) == 5 and sys.argv[1] == "--collector":
        run_child(Path(sys.argv[2]), sys.argv[3], int(sys.argv[4]))
    else:
        unittest.main()

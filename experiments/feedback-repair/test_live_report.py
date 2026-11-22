"""Check the study report and decision on a synthetic ledger without contacting a provider."""

import copy
from decimal import Decimal
import sqlite3
import unittest

import corpus
import live_study
import recorder


class LiveReportTests(unittest.TestCase):
    """Use the published assignment identities and privately held, synthetic observations."""

    def setUp(self):
        """Enroll a fresh in-memory ledger with the frozen 48-assignment denominator."""
        self.db = sqlite3.connect(":memory:")
        self.db.row_factory = sqlite3.Row
        self.addCleanup(self.db.close)
        self.members = corpus.members()
        self.freeze = {"id": "report-test", "assignments": [
            [split, member, arm] for split, member in self.members for arm in recorder.ARMS
        ]}
        live_study.initialize(self.db, self.freeze)
        self.held = [member for split, member in self.members if split == "held_out"]

    def observe(self, member, arm, outcome, *, claim=None, mandatory=False,
                duration=1000, cost=1400, running=False):
        """Insert one synthetic observation with an inseparable native-usage row or reservation."""
        cell = self.db.execute("SELECT id FROM cells WHERE member=? AND arm=?", (member, arm)).fetchone()[0]
        scored = outcome in ("verified", "rejected")
        evidence = {"status": outcome, "elapsed_ns": duration, "claimed_complete": claim,
                    "mandatory_failure": mandatory, "tool_calls": 3, "evaluator_calls": int(scored),
                    "selected": 0 if claim is not None else None,
                    "effect": "uncertain" if running else ("observed" if scored else "none"),
                    "candidates": [{"reply_hex": "private-test-payload"}],
                    "evaluation": {"reasons": [] if outcome == "verified" else ["final_configuration_incorrect"]} if scored else None}
        usage = {"input_tokens": (cost - 1000) // 2, "output_tokens": 100, "list_microusd": cost}
        encoded = recorder.encoded(usage)
        with self.db:
            self.db.execute("UPDATE cells SET state=?,started='start',ended=?,last_step=?,evidence=? WHERE id=?",
                            ("running" if running else "finished", None if running else "end",
                             "selection" if running else "closed", recorder.encoded(evidence), cell))
            self.db.execute("INSERT INTO calls(cell,ordinal,state,reserved,wire,started,usage) VALUES(?,1,?,?,?,'start',?)",
                            (cell, "reserved" if running else "settled", encoded, b"private-test-wire",
                             None if running else encoded))

    def record_billing(self, discount=0, fractional=False):
        """Construct matching synthetic billing totals with optional discounted dollars."""
        totals = live_study.charges(self.db)
        list_micro = totals["list_microusd"]
        billed = Decimal(list_micro) - (Decimal("0.01") if fractional else discount)
        billing = dict(totals, billed_microusd=None if fractional else int(billed),
                       billed_cents=format(billed / 10000, ".6f"),
                       all_rows_at_list=not discount and not fractional, capture={"fixture": True})
        with self.db:
            self.db.execute("INSERT OR REPLACE INTO metadata VALUES('billing', ?)", (recorder.encoded(billing),))
        return billing

    def complete_held_out(self):
        """Give the lesson arm exactly two additional repairs over each required comparator."""
        for index, member in enumerate(self.held):
            for arm, count in (("lesson", 4), ("raw", 2), ("resampling", 2), ("revision", 8)):
                self.observe(member, arm, "verified" if index < count else "rejected", claim=True)
        self.record_billing()

    def test_unfunded_ledger_keeps_all_assignments_and_undefined_rates(self):
        """A fresh ledger reports zero settled calls, no billing and no comparison."""
        result = live_study.report(self.db, self.freeze)
        self.assertEqual(result["states"], {"unattempted": 48, "running": 0, "finished": 0})
        self.assertEqual(len(result["assignments"]), 48)
        self.assertEqual(result["settled_usage"]["requests"], 0)
        self.assertEqual(result["maximum_committed_usage"]["list_microusd"], 0)
        self.assertIsNone(result["billing"])
        self.assertEqual(result["decision"]["state"], "unresolved")
        for split, families in (("development", 2), ("validation", 2), ("held_out", 8)):
            for arm in recorder.ARMS:
                m = result["outcomes"][split][arm]
                self.assertEqual((m["assigned"], m["attempted"], m["scored"]), (families, 0, 0))
                self.assertIsNone(m["verified_per_attempt"])
                self.assertIsNone(m["false_completions_per_claim"])
                self.assertIsNone(m["median_billed_microusd_finished"])
        self.assertIsNone(result["false_blocks"]["rate"])
        self.assertIsNone(result["escapes"]["rate"])

    def test_false_claim_denominators_include_partial_and_unscored_work(self):
        """A halfway-through placement and a timeout leave their explicit claims unresolved."""
        values = [("verified", True), ("rejected", True), ("rejected", False),
                  ("started", True), ("malformed_response", None), ("timeout", True)]
        for member, (outcome, claim) in zip(self.held, values):
            self.observe(member, "raw", outcome, claim=claim, running=outcome == "started",
                         mandatory=None if outcome in ("started", "timeout") else False)
        result = live_study.report(self.db, self.freeze)
        m = result["outcomes"]["held_out"]["raw"]
        self.assertEqual((m["assigned"], m["attempted"], m["finished"], m["scored"]), (8, 6, 5, 3))
        self.assertEqual((m["claimed"], m["unclaimed"], m["unknown_claim"], m["unresolved_claim"]), (4, 1, 1, 2))
        self.assertEqual((m["false_completions"], m["false_completions_per_claim"], m["false_completions_per_attempt"]),
                         (1, 1/4, 1/6))
        self.assertEqual((m["verified_per_attempt"], m["mandatory_unknown"], m["uncertain_placements"]), (1/6, 2, 1))
        self.assertEqual(result["unsettled_reservations"], 1)
        self.assertEqual(result["settled_usage"]["requests"], 5)
        self.assertEqual(result["maximum_committed_usage"]["requests"], 6)
        self.assertEqual(result["decision"]["state"], "unresolved")
        self.assertNotIn("private-test", str(result))

    def test_discounted_bill_blocks_per_cell_attribution(self):
        """A real-valued aggregate discount can't be assigned to a particular strategy."""
        self.complete_held_out()
        for discount, fractional in ((1, False), (0, True)):
            with self.subTest(fractional=fractional):
                self.record_billing(discount, fractional)
                result = live_study.report(self.db, self.freeze)
                m = result["outcomes"]["held_out"]["lesson"]
                self.assertEqual(m["median_list_microusd_finished"], 1400)
                self.assertIsNone(m["median_billed_microusd_finished"])
                self.assertIs(result["billing"]["all_rows_at_list"], False)
                self.assertEqual(result["decision"]["state"], "unresolved")
        mismatched = self.record_billing()
        mismatched["requests"] += 1
        with self.db:
            self.db.execute("UPDATE metadata SET value=? WHERE name='billing'", (recorder.encoded(mismatched),))
        with self.assertRaises(live_study.live_provider.ProviderError):
            live_study.report(self.db, self.freeze)

    def test_eight_paired_thresholds_missing_evidence_and_failure_precedence(self):
        """Require both gains, both median ceilings and known mandatory outcomes on the same eight families."""
        self.complete_held_out()
        base = live_study.report(self.db, self.freeze)
        self.assertEqual(base["decision"], {"state": "consider_larger_test", "additional_repairs_vs_raw": 2,
                         "additional_repairs_vs_resampling": 2, "median_cost_delta_microusd": 0,
                         "median_latency_delta_ns": 0})
        billing = base["billing"]
        rows = base["assignments"]
        for comparator in ("raw", "resampling"):
            trial = copy.deepcopy(rows)
            earlier = next(r for r in trial if r["arm"] == comparator and r["outcome"] == "rejected")
            earlier["outcome"] = "verified"
            self.assertEqual(live_study.decision(trial, billing)["state"], "threshold_not_met")
        for metric in ("cost", "latency"):
            for count in (3, 4):
                with self.subTest(metric=metric, increased=count):
                    trial = copy.deepcopy(rows)
                    lesson = [r for r in trial if r["arm"] == "lesson" and r["split"] == "held_out"]
                    for row in lesson[:count]:
                        if metric == "cost":
                            row["maximum_committed_usage"]["list_microusd"] += 200
                        else:
                            row["elapsed_ns"] += 200
                    decision = live_study.decision(trial, billing)
                    self.assertEqual(decision["state"], "threshold_not_met" if count == 4 else "consider_larger_test")
        member = self.held[-1]
        slot = next(r for r in rows if r["member"] == member and r["arm"] == "lesson")
        for changes in ({"state": "unattempted", "outcome": None}, {"outcome": "timeout"},
                        {"outcome": "incomplete"}, {"mandatory_failure": None}):
            trial = [dict(r, **changes) if r is slot else r for r in rows]
            self.assertEqual(live_study.decision(trial, billing)["state"], "unresolved")
        self.assertEqual(live_study.decision([r for r in rows if r is not slot], billing)["state"], "unresolved")
        marked = [dict(r, mandatory_failure=True) if r is slot else r for r in rows]
        self.assertEqual(live_study.decision(marked, None)["state"], "reject")
        dev = next(r for r in rows if r["split"] == "development" and r["arm"] == "lesson")
        measured = [dict(r, state="finished", outcome="rejected", mandatory_failure=True) if r is dev else r for r in rows]
        self.assertEqual(live_study.decision(measured, billing)["state"], "reject")

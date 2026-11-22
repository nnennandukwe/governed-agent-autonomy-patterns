"""Recalculate the proposed feedback study's resource limits and provider charges offline."""

import json
import unittest
from decimal import Decimal
from pathlib import Path


def dollars(microusd):
    """Format an integer microdollar charge in the guide's exact decimal units."""
    return f"${Decimal(microusd) / 1_000_000:f}"


class BudgetTests(unittest.TestCase):
    """Check the dated price and the allocation separately from spending authority."""

    @classmethod
    def setUpClass(cls):
        """Load the same schedule that the study manifest binds."""
        cls.budget = json.loads(Path(__file__).with_name("budget.json").read_bytes())

    def test_every_arm_and_split_is_charged(self):
        """Include every assigned cell and the independent scorer in the study ceilings."""
        allocation = self.budget["allocation"]
        self.assertEqual(
            allocation["families_by_split"], {"development": 2, "validation": 2, "held_out": 8}
        )
        study_design = json.loads(Path(__file__).with_name("study.json").read_bytes())
        self.assertEqual(allocation["arms"], [arm["id"] for arm in study_design["arms"]])
        self.assertEqual(allocation["arms"], ["raw", "revision", "lesson", "resampling"])
        self.assertEqual(allocation["instances_per_family"], 1)
        self.assertEqual(allocation["repetitions_per_arm"], 1)
        cells = [
            (split, family, arm)
            for split, families in allocation["families_by_split"].items()
            for family in range(families)
            for arm in allocation["arms"]
        ]
        limits = self.budget["per_cell_limits"]
        study = self.budget["study_limits"]
        self.assertEqual(study["trials"], 48)
        self.assertEqual(len(set(cells)), study["trials"])
        expected = {
            "provider_calls": 2,
            "total_tokens": 12_000,
            "strategy_tool_calls": 4,
            "evaluator_calls": 1,
            "elapsed_seconds": 300,
            "candidates": 2,
            "subagent_calls": 0,
        }
        for resource, per_cell in expected.items():
            with self.subTest(resource=resource):
                self.assertEqual(limits[resource], per_cell)
                self.assertEqual(study[resource], len(cells) * per_cell)

    def test_price_and_cost_examples(self):
        """Price full utilization and a mixed-token example across all four arms."""
        price = self.budget["price_snapshot"]
        self.assertEqual((price["checked_on"], price["model"]), ("2026-11-21", "claude-sonnet-5"))
        self.assertEqual((self.budget["currency"], price["service_tier"]), ("USD", "standard_only"))
        input_rate = price["input_microusd_per_token"]
        output_rate = price["output_microusd_per_token"]
        self.assertEqual((input_rate, output_rate), (2, 10))
        estimates = self.budget["estimates"]
        worked = estimates["worked_cell"]
        token_limit = self.budget["per_cell_limits"]["total_tokens"]
        self.assertEqual(worked["input_tokens"] + worked["output_tokens"], token_limit)
        cell_charge = worked["input_tokens"] * input_rate + worked["output_tokens"] * output_rate
        self.assertEqual(worked["provider_cost_microusd"], cell_charge)
        study = self.budget["study_limits"]
        self.assertEqual(estimates["worked_study_microusd"], cell_charge * study["trials"])
        self.assertEqual(estimates["full_input_study_microusd"], study["total_tokens"] * input_rate)
        self.assertEqual(
            estimates["upper_study_microusd"], study["total_tokens"] * max(input_rate, output_rate)
        )
        self.assertLess(estimates["upper_study_microusd"], study["proposed_cost_stop_microusd"])

    def test_published_cost_table_matches_schedule(self):
        """Recalculate each split and compare its cells, calls, tokens and charges with the guide."""
        guide = Path(__file__).with_name("COST.md").read_text(encoding="utf-8")
        table_lines = [line for line in guide.splitlines() if line.startswith("|")]
        columns = [cell.strip() for cell in table_lines[0].strip("|").split("|")]
        self.assertEqual(
            columns,
            ["Split", "Cells", "Provider calls at most", "Combined tokens at most",
             "All input", "Example mix", "Upper charge"],
        )
        allocation = self.budget["allocation"]
        instances = allocation["instances_per_family"] * allocation["repetitions_per_arm"]
        cells_per_family = instances * len(allocation["arms"])
        families = allocation["families_by_split"]
        split_cells = {name: count * cells_per_family for name, count in families.items()}
        split_cells["total"] = sum(split_cells.values())
        limits = self.budget["per_cell_limits"]
        price = self.budget["price_snapshot"]
        input_rate = price["input_microusd_per_token"]
        max_rate = max(input_rate, price["output_microusd_per_token"])
        worked = self.budget["estimates"]["worked_cell"]
        expected = []
        for split, cells in split_cells.items():
            tokens = cells * limits["total_tokens"]
            expected.append([
                split.replace("_", "-").capitalize(), str(cells), str(cells * limits["provider_calls"]),
                f"{tokens:,}", dollars(tokens * input_rate),
                dollars(cells * worked["provider_cost_microusd"]), dollars(tokens * max_rate),
            ])
        actual = [
            [cell.strip().strip("*") for cell in line.strip("|").split("|")]
            for line in table_lines[2:]
        ]
        self.assertEqual(actual, expected)
        stop = dollars(self.budget["study_limits"]["proposed_cost_stop_microusd"])
        self.assertIn(f"**{stop}** spending stop", guide)

    def test_cost_estimates_grant_no_calls(self):
        """Keep unrun cost unknown and the local scorer outside the provider bill."""
        self.assertEqual(
            self.budget["authorization"], {"provider_calls": 0, "provider_cost_microusd": 0}
        )
        self.assertEqual(self.budget["study_limits"]["proposed_cost_stop_microusd"], 25_000_000)
        estimates = self.budget["estimates"]
        self.assertEqual(estimates["offline_provider_cost_microusd"], 0)
        self.assertEqual(estimates["provider_calls_per_evaluator"], 0)
        self.assertIsNone(estimates["actual_trial_provider_cost_microusd"])


if __name__ == "__main__":
    unittest.main()

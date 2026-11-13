"""Check bounded numeric execution and replayable independent verifier samples."""
import importlib.util
import math
from pathlib import Path
import unittest

ENTRY = Path(__file__).resolve().parents[1] / 'src/coding/verify.py'
spec = importlib.util.spec_from_file_location('bounded_shipping_verifier', ENTRY)
verifier = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verifier)

GOOD = """def shipping_quote(quantity, unit_price):
    if quantity <= 0:
        raise ValueError('quantity must be positive')
    subtotal = quantity * unit_price
    if quantity >= 5:
        subtotal = subtotal * 0.9
    if subtotal < 100:
        subtotal = subtotal + 7
    return round(subtotal, 2)
"""

class VerifierTests(unittest.TestCase):
    def test_round_and_boolean_growth_stay_in_the_float_domain(self):
        """Prevent arbitrary-precision growth through both admitted numeric sources."""
        for seed in ['round(2)', '(quantity > 0) + (quantity > 0)']:
            source = ('def shipping_quote(quantity, unit_price):\n    huge = ' + seed + '\n'
                      + '    huge *= huge\n' * 8 + '    return huge\n')
            candidate = verifier.compile_candidate(source)
            value = candidate(1.0, 1.0)
            self.assertIs(type(value), float)
            self.assertTrue(math.isfinite(value))
    def test_repeated_squaring_cant_allocate_a_growing_integer(self):
        """Keep a longer admitted arithmetic program bounded as its value overflows."""
        source = ('def shipping_quote(quantity, unit_price):\n    huge = round(2)\n'
                  + '    huge = huge * huge\n' * 24 + '    return huge\n')
        candidate = verifier.compile_candidate(source)
        self.assertIs(type(candidate(1.0, 1.0)), float)
        self.assertEqual(verifier.observe(candidate, 1, 1)['error'], 'non_finite_or_non_numeric_result')
    def test_source_cant_access_host_helpers_or_oracles(self):
        """Reject names and calls that could reach the verifier's trusted namespace."""
        for expression in ['_gaap_float(quantity)', 'reference_quote(quantity, unit_price)',
                           'challenge_inputs(quantity)']:
            with self.assertRaises(ValueError):
                verifier.compile_candidate('def shipping_quote(quantity, unit_price):\n    return ' + expression + '\n')
    def test_host_samples_are_replayable_per_request_and_change_with_the_seed(self):
        """Record deterministic samples while preventing caller-chosen static vectors."""
        first = list(verifier.challenge_inputs('host-request-A'))
        self.assertEqual(len(first), 32)
        self.assertEqual(first, list(verifier.challenge_inputs('host-request-A')))
        self.assertNotEqual(first, list(verifier.challenge_inputs('host-request-B')))
    def test_known_case_lookup_fails_the_host_seeded_samples(self):
        """Refuse a function that memorizes every published fixture answer."""
        source = """def shipping_quote(quantity, unit_price):
    if quantity <= 0:
        raise ValueError
    if quantity == 1:
        if unit_price == 20:
            return 27
        if unit_price == 100:
            return 100
    if quantity == 2:
        if unit_price == 20:
            return 47
    if quantity == 4:
        if unit_price == 20:
            return 87
    if quantity == 5:
        if unit_price == 20:
            return 97
        if unit_price == 25:
            return 112.5
    if quantity == 6:
        if unit_price == 19.99:
            return 107.95
    return 0
"""
        result = verifier.evaluate(source, [(1, 20), (2, 20), (4, 20), (5, 20),
            (5, 25), (1, 100), (6, 19.99), (0, 20), (-1, 20)], 'host-request-A')
        self.assertEqual(result['observations'][0]['value'], 27)
        self.assertEqual(result['observations'][1]['value'], 47)
        self.assertTrue(any(probe['observed'].get('value') != probe['expected'] for probe in result['probes']))
    def test_correct_function_matches_the_independent_sample_expectations(self):
        """Keep ordinary task code valid across the recorded host challenge inputs."""
        result = verifier.evaluate(GOOD, [(2, 20)], 'host-request-A')
        self.assertEqual(result['observations'][0]['value'], 47)
        self.assertTrue(all(probe['observed'].get('value') == probe['expected'] for probe in result['probes']))

if __name__ == '__main__':
    unittest.main()

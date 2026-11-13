"""Acceptance evidence stays fail-closed under optimized Python."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import unittest

ENTRY = Path(__file__).resolve().parents[1] / 'check_goose.py'
spec = importlib.util.spec_from_file_location('submission_acceptance', ENTRY)
acceptance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(acceptance)

class AcceptanceEvidenceTests(unittest.TestCase):
    def fixtures(self):
        initial = {'files': {'deployment.json': 'disabled'}}
        final = {'files': {'shipping.py': acceptance.GOOD, 'deployment.json': 'disabled'}, 'runs': [
            {'receipt': {'body': {'terminal_status': 'completed'}}, 'verification': {'passed': True, 'cases': [None] * 8}, 'model_usage': None, 'terminal_receipt_verified': True},
            {'receipt': {'body': {'terminal_status': 'blocked'}}, 'protected_effect_results': [{'body': {'execution_status': 'denied'}}], 'model_usage': None, 'terminal_receipt_verified': True},
        ]}
        return final, initial
    def test_invalid_outcomes_cant_emit_a_pass_with_optimization(self):
        final, initial = self.fixtures()
        self.assertEqual(len(acceptance.validate_outcomes(final, initial)), 2)
        invalid = copy.deepcopy(final)
        invalid['runs'][0]['receipt']['body']['terminal_status'] = 'blocked'
        script = ("import importlib.util,json,sys; "
            "spec=importlib.util.spec_from_file_location('acceptance',sys.argv[1]); "
            "module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module); "
            "final,initial=json.load(sys.stdin);module.validate_outcomes(final,initial)")
        result = subprocess.run([sys.executable, '-O', '-B', '-c', script, str(ENTRY)],
            input=json.dumps([invalid, initial]), text=True, capture_output=True, timeout=5)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Code run failed completion', result.stderr)
    def test_changed_files_usage_and_receipt_validation_are_rejected(self):
        for mutate in [lambda final: final['files'].__setitem__('deployment.json', 'enabled'),
                       lambda final: final['runs'][0].__setitem__('model_usage', {'tokens': 0}),
                       lambda final: final['runs'][0].__setitem__('terminal_receipt_verified', False)]:
            final, initial = self.fixtures()
            mutate(final)
            with self.assertRaises(ValueError):
                acceptance.validate_outcomes(final, initial)

if __name__ == '__main__':
    unittest.main()

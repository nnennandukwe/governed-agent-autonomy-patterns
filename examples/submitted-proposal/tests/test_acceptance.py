"""Acceptance evidence stays fail-closed under optimized Python."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ENTRY = Path(__file__).resolve().parents[1] / 'check_goose.py'
spec = importlib.util.spec_from_file_location('submission_acceptance', ENTRY)
acceptance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(acceptance)

class AcceptanceEvidenceTests(unittest.TestCase):
    def fixtures(self):
        """Supply explicit test-only acceptance outcomes and unchanged deployment data."""
        initial = {'files': {'deployment.json': 'disabled'}}
        final = {'files': {'shipping.py': acceptance.GOOD, 'deployment.json': 'disabled'}, 'runs': [
            {'receipt': {'body': {'terminal_status': 'completed'}}, 'verification': {'passed': True, 'cases': [None] * 41, 'fixed_case_count': 9, 'sample_count': 32}, 'model_usage': None, 'terminal_receipt_verified': True},
            {'receipt': {'body': {'terminal_status': 'blocked'}}, 'protected_effect_results': [{'body': {'execution_status': 'denied'}}], 'model_usage': None, 'terminal_receipt_verified': True},
        ]}
        return final, initial
    def test_invalid_outcomes_cant_emit_a_pass_with_optimization(self):
        """Run optimized Python and require invalid completion evidence to fail."""
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
        """Reject changed deployment data, invented usage and invalid receipt status."""
        for mutate in [lambda final: final['files'].__setitem__('deployment.json', 'enabled'),
                       lambda final: final['runs'][0].__setitem__('model_usage', {'tokens': 0}),
                       lambda final: final['runs'][0].__setitem__('terminal_receipt_verified', False)]:
            final, initial = self.fixtures()
            mutate(final)
            with self.assertRaises(ValueError):
                acceptance.validate_outcomes(final, initial)

class ClientCaptureTests(unittest.TestCase):
    def test_timeout_retains_output_transcript_and_failure_metadata(self):
        """Exercise a real hanging child and keep diagnostics without a pass record."""
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary).resolve()
            transcript = [{'fixture': 'timeout evidence'}]
            invocation = [sys.executable, '-u', '-c',
                "import sys,time;print('partial stdout');print('partial stderr',file=sys.stderr);time.sleep(60)"]
            with self.assertRaises(TimeoutError):
                acceptance.capture_goose(invocation, output, {}, output, transcript, timeout=0.2)
            self.assertIn('partial stdout', (output / 'goose-stdout.txt').read_text())
            self.assertIn('partial stderr', (output / 'goose-stderr.txt').read_text())
            self.assertEqual(json.loads((output / 'provider-transcript.json').read_text()), transcript)
            metadata = json.loads((output / 'client-execution.json').read_text())
            self.assertTrue(metadata['timed_out'])
            self.assertNotEqual(metadata['returncode'], 0)
            self.assertFalse((output / 'verification.json').exists())
    def test_failed_client_retains_diagnostics_and_its_actual_exit_status(self):
        """Keep non-timeout failures inspectable without replacing their exit code."""
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary).resolve()
            invocation = [sys.executable, '-u', '-c', "print('failed client');raise SystemExit(7)"]
            returncode = acceptance.capture_goose(invocation, output, {}, output, [], timeout=2)
            self.assertEqual(returncode, 7)
            self.assertIn('failed client', (output / 'goose-stdout.txt').read_text())
            self.assertFalse(json.loads((output / 'client-execution.json').read_text())['timed_out'])


if __name__ == '__main__':
    unittest.main()

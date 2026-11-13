"""Acceptance evidence stays fail-closed under optimized Python."""
import copy
import contextlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
from types import SimpleNamespace

ENTRY = Path(__file__).resolve().parents[1] / 'check_goose.py'
spec = importlib.util.spec_from_file_location('submission_acceptance', ENTRY)
acceptance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(acceptance)

class AcceptanceEvidenceTests(unittest.TestCase):
    def fixtures(self):
        """Supply explicit test-only acceptance outcomes and unchanged deployment data."""
        initial = {'files': {'deployment.json': 'disabled'}}
        final = {'files': {'shipping.py': acceptance.GOOD, 'deployment.json': 'disabled'}, 'runs': [
            {'receipt': {'body': {'terminal_status': 'completed'}}, 'verification': {'passed': True, 'cases': [None] * 41, 'fixed_case_count': 9, 'sample_count': 32}, 'model_usage': None, 'publication_durability': None, 'terminal_receipt_verified': True},
            {'receipt': {'body': {'terminal_status': 'blocked'}}, 'protected_effect_results': [{'body': {'execution_status': 'denied'}}], 'model_usage': None, 'publication_durability': None, 'terminal_receipt_verified': True},
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
                       lambda final: final['runs'][0].__setitem__('publication_durability', 'confirmed'),
                       lambda final: final['runs'][0].__setitem__('terminal_receipt_verified', False)]:
            final, initial = self.fixtures()
            mutate(final)
            with self.assertRaises(ValueError):
                acceptance.validate_outcomes(final, initial)

class ClientCaptureTests(unittest.TestCase):
    def test_real_pin_refusal_keeps_runner_diagnostics_before_host_start(self):
        """Use real hashing to reject a fixture executable before any host command."""
        with tempfile.TemporaryDirectory() as temporary:
            parent = Path(temporary).resolve()
            candidate = parent / 'untrusted-goose'
            candidate.write_bytes(b'This file is a test fixture, not a Goose executable.')
            output = parent / 'evidence'
            with self.assertRaisesRegex(ValueError, 'pinned 1.54.0 artifact'):
                acceptance.run(candidate, output)
            error = json.loads((output / 'runner-error.json').read_text())
            self.assertEqual(error['error_type'], 'ValueError')
            self.assertIn('differs from the pinned', error['message'])
            self.assertFalse((output / 'coding-init-execution.json').exists())
            self.assertFalse((output / 'verification.json').exists())

    def test_initialization_and_spawn_failures_keep_diagnostics(self):
        """Mock pin lookup while exercising actual CLI refusal and missing-binary launch."""
        for missing_binary in [False, True]:
            with self.subTest(missing_binary=missing_binary), tempfile.TemporaryDirectory() as temporary:
                parent = Path(temporary).resolve()
                root = parent / 'session'
                root.mkdir()
                sentinel = root / 'operator-notes.txt'
                sentinel.write_text('preserved operator work')
                output = parent / 'evidence'
                pins = SimpleNamespace(PINS={(acceptance.platform.system(),
                    acceptance.platform.machine()): ('fixture', 'fixture', 'fixture-sha')},
                    digest=lambda _path: 'fixture-sha')
                loader = SimpleNamespace(loader=SimpleNamespace(exec_module=lambda _module: None))
                binary = parent / 'missing-binary' if missing_binary else acceptance.BINARY
                expected = FileNotFoundError if missing_binary else subprocess.CalledProcessError
                with mock.patch.object(acceptance.importlib.util, 'spec_from_file_location', return_value=loader), \
                     mock.patch.object(acceptance.importlib.util, 'module_from_spec', return_value=pins), \
                     mock.patch.object(acceptance.tempfile, 'TemporaryDirectory',
                         return_value=contextlib.nullcontext(str(parent))), \
                     mock.patch.object(acceptance, 'BINARY', binary):
                    with self.assertRaises(expected):
                        acceptance.run(parent / 'pin-fixture', output)
                self.assertTrue((output / 'coding-init-stdout.txt').is_file())
                self.assertTrue((output / 'coding-init-stderr.txt').is_file())
                metadata = json.loads((output / 'coding-init-execution.json').read_text())
                self.assertEqual(metadata['returncode'], None if missing_binary else 1)
                self.assertFalse(metadata['timed_out'])
                self.assertEqual(json.loads((output / 'runner-error.json').read_text())['error_type'],
                    expected.__name__)
                if not missing_binary:
                    self.assertIn('File exists', (output / 'coding-init-stderr.txt').read_text())
                self.assertEqual(sentinel.read_text(), 'preserved operator work')
                self.assertFalse((output / 'verification.json').exists())

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

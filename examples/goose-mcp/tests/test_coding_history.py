"""Real Rust consumer and HTTP integration checks for persistent run history."""
from contextlib import contextmanager
import http.client
from http.server import ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import coding_demo


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


@unittest.skipUnless(coding_demo.legacy.BINARY.is_file(),
                     'Build examples/goose-mcp/Cargo.toml before Rust integration tests')
class CodingHistoryTests(unittest.TestCase):
    def command(self, command, root, value=None):
        """Use the production CLI and require a successfully parsed result."""
        result = subprocess.run(
            [str(coding_demo.legacy.BINARY), command, str(root)],
            input=json.dumps(value) if value is not None else None,
            text=True, capture_output=True, timeout=20,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    @contextmanager
    def inspector(self, root, port=0):
        """Serve actual validated records; no mock store or model is involved."""
        server = ThreadingHTTPServer(
            ('127.0.0.1', port), coding_demo.handler(root, 'persistent-history-token'),
        )
        thread = threading.Thread(
            target=server.serve_forever, kwargs={'poll_interval': 0.01}, daemon=True,
        )
        thread.start()
        try:
            yield server.server_port
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)
            self.assertFalse(thread.is_alive(), 'Inspector server did not stop')

    def state(self, port):
        """Fetch with a new connection, as a refreshed browser would."""
        connection = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
        try:
            connection.request('GET', '/persistent-history-token/state')
            response = connection.getresponse()
            body = response.read()
            self.assertEqual(response.status, 200, body.decode())
            self.assertEqual(response.getheader('Cache-Control'), 'no-store')
            return json.loads(body)
        finally:
            connection.close()

    def test_restart_refresh_and_follow_up_preserve_receipts_and_deduplication(self):
        """Accumulate real gated effects across HTTP restarts without replaying writes."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve() / 'coding'
            initial = self.command('coding-init', root)
            implementation = {
                'path': 'shipping.py', 'content': GOOD,
                'plan': 'Implement shipping pricing and verify its acceptance cases.',
                'base_digest': initial['subject_digest'],
            }
            completed = self.command('coding-submit', root, implementation)
            self.assertEqual(completed['receipt']['body']['terminal_status'], 'completed')
            self.assertTrue(completed['verification']['passed'])
            self.assertEqual(len(completed['verification']['cases']), 8)

            current = self.command('coding-inspect', root)
            deployment = json.loads(current['files']['deployment.json'])
            deployment['shipping_enabled'] = True
            denied = self.command('coding-submit', root, {
                'path': 'deployment.json', 'content': json.dumps(deployment),
                'plan': 'Enable shipping while preserving the other deployment settings.',
                'base_digest': current['subject_digest'],
            })
            self.assertEqual(denied['receipt']['body']['terminal_status'], 'blocked')
            self.assertEqual(
                denied['protected_effect_results'][0]['body']['execution_status'], 'denied',
            )
            original_runs = [completed, denied]

            with self.inspector(root) as port:
                before_restart = self.state(port)
                self.assertEqual(before_restart['runs'], original_runs)
                self.assertEqual(before_restart['files']['shipping.py'], GOOD)
                self.assertEqual(
                    before_restart['files']['deployment.json'], initial['files']['deployment.json'],
                )
                self.assertIsNone(before_restart['inflight'])

            with self.inspector(root, port):
                restored = self.state(port)
                self.assertEqual(restored, before_restart)
                follow_up = {
                    'path': 'shipping.py',
                    'content': GOOD.replace('quantity must be positive', 'quantity must exceed zero'),
                    'plan': 'Clarify the invalid-quantity error and reverify the pricing rules.',
                    'base_digest': restored['subject_digest'],
                }
                appended = self.command('coding-submit', root, follow_up)
                self.assertEqual(appended['receipt']['body']['terminal_status'], 'completed')
                self.assertTrue(appended['verification']['passed'])
                after_follow_up = self.state(port)
                self.assertEqual(after_follow_up['runs'], original_runs + [appended])
                self.assertEqual(after_follow_up['files']['shipping.py'], follow_up['content'])
                self.assertEqual(after_follow_up['limits'], before_restart['limits'])

            with self.inspector(root, port):
                self.assertEqual(self.state(port), after_follow_up)
                replay = self.command('coding-submit', root, follow_up)
                self.assertTrue(replay.pop('duplicate'))
                self.assertEqual(replay, {key: value for key, value in appended.items()
                                          if key != 'duplicate'})
                # Exact replay returns the old receipt and cannot create a fourth run.
                final = self.state(port)
                self.assertEqual(final, after_follow_up)
                self.assertTrue(all(run['terminal_receipt_verified'] for run in final['runs']))
                self.assertEqual(len(final['runs']), 3)
                self.assertEqual(final['runs'][:2], original_runs)
                self.assertEqual(final['limits']['effects_per_run'], 1)
                self.assertEqual(final['limits']['accounted_calls_per_run'], 2)
                self.assertEqual(final['files']['deployment.json'], initial['files']['deployment.json'])

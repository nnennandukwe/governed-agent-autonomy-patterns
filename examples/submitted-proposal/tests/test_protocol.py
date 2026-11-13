"""Independent MCP JSON-RPC client and real CLI history checks; no model calls."""
import fcntl
import json
from pathlib import Path
import queue
import select
import subprocess
import threading
import time
import tempfile
import unittest

BINARY = Path(__file__).resolve().parents[1] / 'target/debug/gaap-submitted-proposal-reference'
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

def cli(command, root, value=None, success=True):
    result = subprocess.run([str(BINARY), command, str(root)],
        input=None if value is None else json.dumps(value), capture_output=True,
        text=True, timeout=20, env={})
    if success and result.returncode:
        raise AssertionError(result.stderr)
    return json.loads(result.stdout) if success else result

class Client:
    """Use only the public MCP wire format, independently of the Rust SDK."""
    def __init__(self, root):
        self.process = subprocess.Popen([str(BINARY), 'coding-serve', str(root)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, env={})
        self.responses = queue.Queue()
        self.pending = {}
        self.sequence = 0
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()
        result = self.request('initialize', {'protocolVersion': '2025-11-25',
            'capabilities': {}, 'clientInfo': {'name': 'independent-python-fixture', 'version': '1'}})
        assert 'protocolVersion' in result, result
        self.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
    def read(self):
        for line in self.process.stdout:
            self.responses.put(json.loads(line))
    def send(self, value):
        self.process.stdin.write(json.dumps(value) + '\n')
        self.process.stdin.flush()
    def start(self, method, params):
        self.sequence += 1
        self.send({'jsonrpc': '2.0', 'id': self.sequence, 'method': method, 'params': params})
        return self.sequence
    def finish(self, identifier):
        deadline = time.monotonic() + 20
        while identifier not in self.pending:
            result = self.responses.get(timeout=max(.01, deadline - time.monotonic()))
            if 'id' in result:
                self.pending[result['id']] = result
        result = self.pending.pop(identifier)
        if 'error' in result:
            raise AssertionError(result)
        return result['result']
    def request(self, method, params):
        return self.finish(self.start(method, params))
    def tool(self, name, arguments=None):
        result = self.request('tools/call', {'name': name, 'arguments': arguments or {}})
        return json.loads(result['content'][0]['text']), result.get('isError', False)
    def active(self):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            result, error = self.tool('run_status')
            assert not error, result
            if result['active_submissions']:
                return
            time.sleep(.01)
        raise AssertionError('Submission never entered the transport')
    def close(self):
        if getattr(self, "closed", False):
            return
        self.closed = True
        if not self.process.stdin.closed:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=2)
            raise
        self.reader.join(timeout=2)
        errors = self.process.stderr.read()
        self.process.stdout.close()
        self.process.stderr.close()
        assert self.process.returncode == 0, errors

class ProtocolTests(unittest.TestCase):
    def setUp(self):
        self.parent = tempfile.TemporaryDirectory()
        self.addCleanup(self.parent.cleanup)
        self.root = Path(self.parent.name).resolve() / 'session'
        self.initial = cli('coding-init', self.root)
        self.client = Client(self.root)
        self.addCleanup(self.client.close)
    def change(self, **overrides):
        value = {'path': 'shipping.py', 'content': GOOD,
            'plan': 'Implement the shipping acceptance cases.',
            'base_digest': self.initial['subject_digest']}
        return dict(value, **overrides)
    def stored(self):
        return cli('coding-inspect', self.root)
    def wait_stored(self):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            state = self.stored()
            if state['runs']:
                return state
            time.sleep(.01)
        raise AssertionError('Worker never stored its outcome')
    def lock(self):
        handle = (self.root / 'operator/lock').open('r+')
        fcntl.flock(handle, fcntl.LOCK_EX)
        self.addCleanup(handle.close)
        return handle
    def test_discovery_completion_denial_restart_and_historical_duplicate(self):
        tools = self.client.request('tools/list', {})['tools']
        self.assertEqual([tool['name'] for tool in tools], ['read_project', 'submit_change', 'run_status'])
        self.assertFalse(tools[1]['inputSchema']['additionalProperties'])
        completed, error = self.client.tool('submit_change', self.change())
        self.assertFalse(error, completed)
        self.assertEqual(completed['receipt']['body']['terminal_status'], 'completed')
        self.assertIsNone(completed['model_usage'])
        self.assertEqual(len(completed['verification']['cases']), 8)
        current, _ = self.client.tool('read_project')
        denied, error = self.client.tool('submit_change', self.change(path='deployment.json',
            content='{"shipping_enabled":true}', base_digest=current['subject_digest']))
        self.assertFalse(error, denied)
        self.assertEqual(denied['protected_effect_results'][0]['body']['execution_status'], 'denied')
        before = self.stored()
        self.assertEqual(before['files']['deployment.json'], self.initial['files']['deployment.json'])
        self.client.close()
        self.client = Client(self.root)
        self.addCleanup(self.client.close)
        replay, error = self.client.tool('submit_change', self.change())
        self.assertFalse(error, replay)
        self.assertTrue(replay.pop('duplicate'))
        self.assertEqual(replay, {key: value for key, value in completed.items() if key != 'duplicate'})
        self.assertEqual(self.stored(), before)
        self.assertNotEqual(completed['request']['run_id'], denied['request']['run_id'])
        failed_init = cli('coding-init', self.root, success=False)
        self.assertNotEqual(failed_init.returncode, 0)
        self.assertEqual(self.stored(), before)
    def test_authority_forgery_and_stale_input_fail_before_admission(self):
        for field in ['approved', 'approval_context', 'verifier_id', 'policy_decision',
                      'terminal_status', 'threadloop_claim', 'run_id', 'request_id']:
            result, error = self.client.tool('submit_change', self.change(**{field: True}))
            self.assertTrue(error, (field, result))
            self.assertIn('Inspect run_status', result['next_action'])
        result, error = self.client.tool('read_project', {'approved': True})
        self.assertTrue(error, result)
        (self.root / 'workspace/shipping.py').write_text('manual change\n')
        result, error = self.client.tool('submit_change', self.change())
        self.assertTrue(error, result)
        self.assertIn('binding', result['error'])
        self.assertEqual(self.stored()['runs'], [])
        self.assertIsNone(self.stored()['inflight'])
        self.assertEqual((self.root / 'workspace/shipping.py').read_text(), 'manual change\n')
    def test_concurrent_duplicate_delivery_dispatches_once(self):
        first = self.client.start('tools/call', {'name': 'submit_change', 'arguments': self.change()})
        second = self.client.start('tools/call', {'name': 'submit_change', 'arguments': self.change()})
        results = [json.loads(self.client.finish(identifier)['content'][0]['text']) for identifier in (first, second)]
        self.assertEqual(sorted(result['duplicate'] for result in results), [False, True])
        self.assertEqual(results[0]['receipt'], results[1]['receipt'])
        self.assertEqual(len(self.stored()['runs']), 1)
    def test_concurrent_different_proposals_dont_overwrite_the_winner(self):
        identifiers = [self.client.start('tools/call', {'name': 'submit_change', 'arguments': value})
            for value in [self.change(), self.change(content=GOOD.replace('positive', 'greater than zero'))]]
        results = [self.client.finish(identifier) for identifier in identifiers]
        self.assertEqual(sum(result.get('isError', False) for result in results), 1)
        self.assertEqual(len(self.stored()['runs']), 1)
    def test_cancellation_while_waiting_for_lock_drains_without_dispatch(self):
        lock = self.lock()
        identifier = self.client.start('tools/call', {'name': 'submit_change', 'arguments': self.change()})
        self.client.active()
        self.client.send({'jsonrpc': '2.0', 'method': 'notifications/cancelled', 'params': {'requestId': identifier}})
        # A subsequent status response bounds delivery of the cancellation notification.
        self.client.tool('run_status')
        fcntl.flock(lock, fcntl.LOCK_UN)
        state = self.wait_stored()
        self.assertEqual(state['runs'][0]['receipt']['body']['terminal_status'], 'interrupted')
        self.assertEqual(state['runs'][0]['protected_effect_results'], [])
        self.assertEqual(state['files']['shipping.py'], self.initial['files']['shipping.py'])
        self.assertTrue(cli('coding-submit', self.root, self.change())['duplicate'])
    def test_disconnect_while_queued_drains_without_replay(self):
        lock = self.lock()
        self.client.start('tools/call', {'name': 'submit_change', 'arguments': self.change()})
        self.client.active()
        self.client.process.stdin.close()
        # The host reports this after observing EOF and requesting cancellation.
        ready, _, _ = select.select([self.client.process.stderr], [], [], 5)
        self.assertTrue(ready, 'Host never observed transport closure')
        self.assertIn('MCP connection closed', self.client.process.stderr.readline())
        fcntl.flock(lock, fcntl.LOCK_UN)
        state = self.wait_stored()
        self.assertEqual(state['runs'][0]['receipt']['body']['terminal_status'], 'interrupted')
        self.assertEqual(state['runs'][0]['protected_effect_results'], [])
        self.assertTrue(cli('coding-submit', self.root, self.change())['duplicate'])
    def test_host_deadline_while_queued_cant_dispatch_later(self):
        lock = self.lock()
        identifier = self.client.start('tools/call', {'name': 'submit_change', 'arguments': self.change()})
        self.client.active()
        time.sleep(10.05)
        fcntl.flock(lock, fcntl.LOCK_UN)
        result = json.loads(self.client.finish(identifier)['content'][0]['text'])
        self.assertEqual(result['receipt']['body']['terminal_status'], 'interrupted')
        self.assertIn('host proposal deadline exceeded', json.dumps(result['receipt']['body']['events']))
        self.assertEqual(result['protected_effect_results'], [])
        self.assertTrue(cli('coding-submit', self.root, self.change())['duplicate'])

if __name__ == '__main__':
    unittest.main()

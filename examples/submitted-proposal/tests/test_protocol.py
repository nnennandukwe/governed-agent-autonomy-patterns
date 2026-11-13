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
    """Invoke the production CLI with a bounded wait and explicit exit checking."""
    result = subprocess.run([str(BINARY), command, str(root)],
        input=None if value is None else json.dumps(value), capture_output=True,
        text=True, timeout=20, env={})
    if success and result.returncode:
        raise AssertionError(result.stderr)
    return json.loads(result.stdout) if success else result

class CliIngressTests(unittest.TestCase):
    def test_cli_caps_bytes_before_parsing_and_preserves_the_session(self):
        """Refuse oversized JSON and padding before admission, then accept the boundary."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve() / 'session'
            status = cli('coding-init', root)
            proposal = {'path': 'shipping.py', 'content': GOOD,
                'plan': 'Implement the shipping task.', 'base_digest': status['subject_digest']}
            encoded = json.dumps(proposal).encode()
            state = root / 'operator/state.json'
            before = state.read_bytes()
            source = (root / 'workspace/shipping.py').read_bytes()
            oversized = [b' ' * (256 * 1024) + encoded,
                json.dumps(dict(proposal, content='x' * (256 * 1024))).encode()]
            for payload in oversized:
                result = subprocess.run([str(BINARY), 'coding-submit', str(root)],
                    input=payload, capture_output=True, timeout=20, env={})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(b'256 KiB byte limit', result.stderr)
                self.assertEqual(state.read_bytes(), before)
                self.assertEqual((root / 'workspace/shipping.py').read_bytes(), source)
                self.assertEqual(cli('coding-inspect', root)['runs'], [])
            boundary = encoded + b' ' * (256 * 1024 - len(encoded))
            result = subprocess.run([str(BINARY), 'coding-submit', str(root)],
                input=boundary, capture_output=True, timeout=20, env={})
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout)['receipt']['body']['terminal_status'], 'completed')


class Client:
    """Use only the public MCP wire format, independently of the Rust SDK."""
    def __init__(self, root):
        """Launch the independent protocol client and complete MCP initialization."""
        self.process = subprocess.Popen([str(BINARY), 'coding-serve', str(root)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, env={})
        self.responses = queue.Queue()
        self.pending = {}
        self.messages = []
        self.sequence = 0
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()
        result = self.request('initialize', {'protocolVersion': '2025-11-25',
            'capabilities': {}, 'clientInfo': {'name': 'independent-python-fixture', 'version': '1'}})
        assert 'protocolVersion' in result, result
        self.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
    def read(self):
        """Collect newline-delimited server messages on a separate reader thread."""
        for line in self.process.stdout:
            message = json.loads(line)
            self.messages.append(message)
            self.responses.put(message)
    def send(self, value):
        """Write and flush one JSON-RPC message to the server."""
        self.process.stdin.write(json.dumps(value) + '\n')
        self.process.stdin.flush()
    def start(self, method, params):
        """Allocate a client correlation ID and send a request without waiting."""
        self.sequence += 1
        self.send({'jsonrpc': '2.0', 'id': self.sequence, 'method': method, 'params': params})
        return self.sequence
    def finish(self, identifier):
        """Collect the matching response within the client deadline."""
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
        """Send a protocol request and return its matching response."""
        return self.finish(self.start(method, params))
    def tool(self, name, arguments=None):
        """Call one public tool and decode its result and error flag."""
        result = self.request('tools/call', {'name': name, 'arguments': arguments or {}})
        return json.loads(result['content'][0]['text']), result.get('isError', False)
    def active(self):
        """Wait until a submitted request reaches the transport handler."""
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            result, error = self.tool('run_status')
            assert not error, result
            if result['active_submissions']:
                return
            time.sleep(.01)
        raise AssertionError('Submission never entered the transport')
    def close(self):
        """Close the connection, drain the process and release its streams once."""
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
        """Create a fresh fixture and independent client for each integration test."""
        self.parent = tempfile.TemporaryDirectory()
        self.addCleanup(self.parent.cleanup)
        self.root = Path(self.parent.name).resolve() / 'session'
        self.initial = cli('coding-init', self.root)
        self.client = Client(self.root)
        self.addCleanup(self.client.close)
    def change(self, **overrides):
        """Build a proposal from the observed subject with optional test overrides."""
        value = {'path': 'shipping.py', 'content': GOOD,
            'plan': 'Implement the shipping acceptance cases.',
            'base_digest': self.initial['subject_digest']}
        return dict(value, **overrides)
    def stored(self):
        """Read the saved ledger through the production inspector."""
        return cli('coding-inspect', self.root)
    def wait_stored(self):
        """Wait for a drained worker to publish its actual outcome."""
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            state = self.stored()
            if state['runs']:
                return state
            time.sleep(.01)
        raise AssertionError('Worker never stored its outcome')
    def lock(self):
        """Hold the real admission lock while exercising queued requests."""
        handle = (self.root / 'operator/lock').open('r+')
        fcntl.flock(handle, fcntl.LOCK_EX)
        self.addCleanup(handle.close)
        return handle
    def test_discovery_completion_denial_restart_and_historical_duplicate(self):
        """Verify discovery, both outcomes and historical duplicate readback after restart."""
        tools = self.client.request('tools/list', {})['tools']
        self.assertEqual([tool['name'] for tool in tools], ['read_project', 'submit_change', 'run_status'])
        self.assertFalse(tools[1]['inputSchema']['additionalProperties'])
        completed, error = self.client.tool('submit_change', self.change())
        self.assertFalse(error, completed)
        self.assertEqual(completed['receipt']['body']['terminal_status'], 'completed')
        self.assertIsNone(completed['model_usage'])
        self.assertEqual(len(completed['verification']['cases']), 41)
        self.assertEqual(completed['verification']['fixed_case_count'], 9)
        self.assertEqual(completed['verification']['sample_count'], 32)
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
        """Reject forged authority and preserve a manual edit before admission."""
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
        """Submit concurrent copies and require exactly one saved execution."""
        first = self.client.start('tools/call', {'name': 'submit_change', 'arguments': self.change()})
        second = self.client.start('tools/call', {'name': 'submit_change', 'arguments': self.change()})
        results = [json.loads(self.client.finish(identifier)['content'][0]['text']) for identifier in (first, second)]
        self.assertEqual(sorted(result['duplicate'] for result in results), [False, True])
        self.assertEqual(results[0]['receipt'], results[1]['receipt'])
        self.assertEqual(len(self.stored()['runs']), 1)
    def test_concurrent_different_proposals_dont_overwrite_the_winner(self):
        """Reject the stale concurrent proposal without replacing the first result."""
        identifiers = [self.client.start('tools/call', {'name': 'submit_change', 'arguments': value})
            for value in [self.change(), self.change(content=GOOD.replace('positive', 'greater than zero'))]]
        results = [self.client.finish(identifier) for identifier in identifiers]
        self.assertEqual(sum(result.get('isError', False) for result in results), 1)
        self.assertEqual(len(self.stored()['runs']), 1)
    def test_cancellation_while_waiting_for_lock_drains_without_dispatch(self):
        """Cancel a queued request and retain an interruption without an effect."""
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
        """Disconnect during admission wait and inspect the drained interruption."""
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
    def test_oversized_frame_is_refused_before_proposal_parsing(self):
        """Reject a large wire frame and resynchronize without admitting a run."""
        oversized = self.client.start('tools/call', {'name': 'submit_change',
            'arguments': self.change(content='x' * (300 * 1024))})
        state, error = self.client.tool('run_status')
        self.assertFalse(error, state)
        self.assertFalse(any(message.get('id') == oversized for message in self.client.messages),
                         'Oversized input reached proposal parsing')
        self.assertEqual(self.stored()['runs'], [])
        result, error = self.client.tool('submit_change', self.change())
        self.assertFalse(error, result)
        self.assertEqual(result['receipt']['body']['terminal_status'], 'completed')
    def test_request_flood_closes_before_extra_work_can_be_admitted(self):
        """Bound outstanding requests while a real session lock holds execution."""
        lock = self.lock()
        for _ in range(9):
            self.client.start('tools/call', {'name': 'submit_change', 'arguments': self.change()})
        ready, _, _ = select.select([self.client.process.stderr], [], [], 2)
        self.assertTrue(ready, 'Transport accepted more than eight outstanding requests')
        self.assertIn('MCP outstanding request limit', self.client.process.stderr.readline())
        fcntl.flock(lock, fcntl.LOCK_UN)
        self.client.close()
        state = self.stored()
        self.assertEqual(state['files'], self.initial['files'])
        self.assertTrue(all(not run['protected_effect_results'] for run in state['runs']))
    def test_duplicate_active_rpc_identity_cant_mix_two_proposals(self):
        """Close ambiguous active RPC IDs without executing either proposal."""
        lock = self.lock()
        identifier = self.client.start('tools/call', {'name': 'submit_change', 'arguments': self.change()})
        self.client.active()
        self.client.send({'jsonrpc': '2.0', 'id': identifier, 'method': 'tools/call',
            'params': {'name': 'submit_change', 'arguments': self.change(content='different')}})
        ready, _, _ = select.select([self.client.process.stderr], [], [], 2)
        self.assertTrue(ready, 'Transport accepted an ambiguous active request ID')
        self.assertIn('MCP duplicate active request ID', self.client.process.stderr.readline())
        fcntl.flock(lock, fcntl.LOCK_UN)
        self.client.close()
        self.assertEqual(self.stored()['files'], self.initial['files'])

    def test_host_deadline_while_queued_cant_dispatch_later(self):
        """Expire a queued proposal and refuse a later write or duplicate replay."""
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

"""Real HTTP checks for the local inspector's read boundary."""
import http.client
from http.server import ThreadingHTTPServer
from pathlib import Path
import sys
import threading
import unittest
from unittest.mock import patch
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import coding_demo


class InspectorTests(unittest.TestCase):
    def setUp(self):
        """Start a real loopback HTTP server with a controlled record source."""
        self.patcher = patch.object(coding_demo, 'inspect', return_value={'runs': [], 'files': {}})
        self.patcher.start()
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), coding_demo.handler(Path('/unused'), 'session-token'))
        self.thread = threading.Thread(target=self.server.serve_forever, kwargs={'poll_interval': 0.01}, daemon=True)
        self.thread.start()

    def tearDown(self):
        """Stop the HTTP server and restore the record source after each test."""
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=1)
        self.patcher.stop()

    def request(self, path, method='GET', headers=None):
        """Issue a real HTTP request and return its status, headers, and body."""
        connection = http.client.HTTPConnection('127.0.0.1', self.server.server_port, timeout=2)
        connection.request(method, path, headers=headers or {})
        response = connection.getresponse()
        result = response.status, dict(response.getheaders()), response.read()
        connection.close()
        return result

    def test_real_assets_and_state_have_restrictive_headers(self):
        """Check that the real assets and JSON responses use restrictive headers."""
        for path in ['', 'app.js', 'style.css', 'state']:
            status, headers, body = self.request('/session-token/' + path)
            self.assertEqual(status, 200)
            self.assertTrue(body)
            self.assertEqual(headers['Cache-Control'], 'no-store')
            self.assertIn("frame-ancestors 'none'", headers['Content-Security-Policy'])
            self.assertNotIn('Access-Control-Allow-Origin', headers)

    def test_foreign_origins_hosts_tokens_and_mutations_are_rejected(self):
        """Prove the HTTP boundary rejects foreign origins and mutation attempts."""
        self.assertEqual(self.request('/session-token/state', headers={'Host': 'attacker.example'})[0], 403)
        self.assertEqual(self.request('/session-token/state', headers={'Origin': 'https://attacker.example'})[0], 403)
        self.assertEqual(self.request('/wrong/state')[0], 404)
        self.assertEqual(self.request('/session-token/../../operator/state.json')[0], 404)
        self.assertEqual(self.request('/session-token/state', method='POST')[0], 501)

    def test_inspection_failure_returns_unavailable_not_empty_success(self):
        """Keep invalid evidence distinct from a valid empty session."""
        with patch.object(coding_demo, 'inspect', side_effect=ValueError('invalid receipt')):
            status, _, body = self.request('/session-token/state')
        self.assertEqual(status, 503)
        self.assertIn(b'could not be inspected', body)

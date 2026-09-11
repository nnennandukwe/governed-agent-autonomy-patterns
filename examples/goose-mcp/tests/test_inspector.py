"""Real HTTP checks for the local inspector's read boundary."""
import http.client
import json
import os
from http.server import ThreadingHTTPServer
from pathlib import Path
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import coding_demo


class MetadataTests(unittest.TestCase):
    def test_startup_rejects_symlink_without_changing_target(self):
        """Preserve an unrelated file when saved inspector metadata is a link."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / 'keep.txt'
            target.write_text('preserve me')
            (root / 'inspector.json').symlink_to(target)
            with patch.object(coding_demo, 'inspect', return_value={}), patch.object(
                ThreadingHTTPServer, 'serve_forever', side_effect=KeyboardInterrupt
            ):
                with self.assertRaises(OSError):
                    coding_demo.serve(root, open_browser=False)
            self.assertEqual(target.read_text(), 'preserve me')

    def test_metadata_replacement_and_failed_commit_preserve_records(self):
        """Publish whole records and retain previous metadata on rename failure."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ['inspector.json', 'rehearsal.json']:
                coding_demo.write_metadata(root, name, {'version': 1})
                coding_demo.write_metadata(root, name, {'version': 2})
                self.assertEqual(json.loads((root / name).read_text()), {'version': 2})
                self.assertEqual((root / name).stat().st_mode & 0o777, 0o600)
                with patch.object(coding_demo.os, 'replace', side_effect=OSError('disk error')):
                    with self.assertRaises(OSError):
                        coding_demo.write_metadata(root, name, {'version': 3})
                self.assertEqual(json.loads((root / name).read_text()), {'version': 2})
            self.assertEqual(sorted(p.name for p in root.iterdir()), ['inspector.json', 'rehearsal.json'])

    def test_symlinks_hardlinks_and_nonfiles_cannot_redirect_metadata(self):
        """Reject unsafe destinations for both launcher metadata records."""
        for name in ['inspector.json', 'rehearsal.json']:
            for kind in ['symlink', 'hardlink', 'directory']:
                with self.subTest(name=name, kind=kind), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    target = root / 'keep.txt'
                    target.write_text('preserve me')
                    destination = root / name
                    if kind == 'symlink':
                        destination.symlink_to(target)
                    elif kind == 'hardlink':
                        os.link(target, destination)
                    else:
                        destination.mkdir()
                    with self.assertRaises(OSError):
                        coding_demo.write_metadata(root, name, {'changed': True})
                    self.assertEqual(target.read_text(), 'preserve me')

    def test_late_symlink_cannot_redirect_atomic_replacement(self):
        """A destination swap at rename can never overwrite the linked target."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / 'keep.txt'
            target.write_text('preserve me')
            replace = os.replace

            def swap_then_replace(source, destination, **kwargs):
                """Inject a native destination swap after the final validation."""
                (root / destination).symlink_to(target)
                replace(source, destination, **kwargs)

            with patch.object(coding_demo.os, 'replace', side_effect=swap_then_replace):
                coding_demo.write_metadata(root, 'inspector.json', {'safe': True})
            self.assertEqual(target.read_text(), 'preserve me')
            self.assertFalse((root / 'inspector.json').is_symlink())
            self.assertEqual(json.loads((root / 'inspector.json').read_text()), {'safe': True})


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

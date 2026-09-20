"""Lifecycle and failure tests for durable launcher selection and real HTTP reuse."""
import concurrent.futures
import http.client
import json
import os
from pathlib import Path
import socket
import stat
import sys
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import coding_demo


class PersistenceTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.repo = Path(self.directory.name)
        self.handles = []
        self.patches = [patch.object(coding_demo, 'REPO', self.repo),
                        patch.object(coding_demo.legacy, 'binary', side_effect=self.binary),
                        patch.object(coding_demo.subprocess, 'run'),
                        patch.object(coding_demo.webbrowser, 'open', return_value=True),
                        patch.object(coding_demo.legacy, 'verify_binary')]
        self.mocks = [p.start() for p in self.patches]
        self.goose, self.browser = self.mocks[2:4]

    def tearDown(self):
        for handle in reversed(self.handles):
            handle.close()
        for p in reversed(self.patches):
            p.stop()
        self.directory.cleanup()

    def binary(self, command, root, **_):
        """Replace only the external Rust process; retain real files and HTTP."""
        if command == 'coding-init':
            root.mkdir()
            (root / 'workspace').mkdir()
            (root / 'operator').mkdir()
            (root / 'operator/state.json').write_text(json.dumps({'runs': [], 'files': {}}))
            return {}
        if command == 'coding-inspect':
            return json.loads((root / 'operator/state.json').read_text())
        raise AssertionError(command)

    def start(self, **kwargs):
        handle = coding_demo.start_session(**kwargs)
        self.handles.append(handle)
        return handle

    def pointer(self):
        return (self.repo / 'demo-runs/current.json').read_bytes()

    def request(self, url, suffix):
        parsed = urlsplit(url)
        connection = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=2)
        try:
            connection.request('GET', parsed.path + suffix)
            response = connection.getresponse()
            return response.status, json.loads(response.read())
        finally:
            connection.close()

    def test_first_start_then_active_resume_preserve_evidence_and_ui(self):
        first = self.start()
        evidence = first.root / 'operator/state.json'
        evidence.write_text('{"runs": [{"id": "accepted-change"}], "files": {}}')
        before = evidence.read_bytes()
        pointer = self.pointer()
        second = self.start()
        self.assertEqual((second.root, second.url), (first.root, first.url))
        self.assertIsNone(second.server)
        self.assertEqual(evidence.read_bytes(), before)
        self.assertEqual(self.pointer(), pointer)
        self.assertEqual(self.request(second.url, 'state')[1]['runs'][0]['id'], 'accepted-change')
        self.assertEqual(self.goose.call_count, 1)
        self.assertEqual(self.browser.call_count, 1)

    def test_stopped_restart_rebinds_saved_url_without_reopening_either_ui(self):
        first = self.start()
        first.close()
        resumed = self.start()
        self.assertEqual((resumed.root, resumed.url), (first.root, first.url))
        self.assertEqual(self.request(resumed.url, 'state')[0], 200)
        self.assertEqual(self.goose.call_count, 1)
        self.assertEqual(self.browser.call_count, 1)

    def test_explicit_new_and_existing_root_adoption_preserve_both_roots(self):
        first = self.start(open_browser=False)
        old_record = (first.root / 'operator/state.json').read_bytes()
        second = self.start(new_session=True, open_browser=False)
        self.assertNotEqual(first.root, second.root)
        adopted = self.start(root=first.root, open_browser=False)
        self.assertEqual(adopted.url, first.url)
        self.assertEqual(json.loads(self.pointer()), {'root': first.root.name})
        self.assertEqual((first.root / 'operator/state.json').read_bytes(), old_record)
        self.assertTrue((second.root / 'operator/state.json').is_file())
        self.assertEqual(self.goose.call_count, 2)

    def test_identity_reuse_does_not_depend_on_valid_inspection(self):
        first = self.start(open_browser=False)
        (first.root / 'operator/state.json').write_text('corrupt')
        status, identity = self.request(first.url, 'identity')
        self.assertEqual(status, 200)
        self.assertEqual(identity['service'], 'gaap-coding-inspector-v1')
        self.assertEqual(identity['root'], str(first.root))
        self.assertEqual(self.request(first.url, 'state')[0], 503)
        self.assertEqual(self.start(open_browser=False).url, first.url)
        with self.assertRaises(ValueError):
            self.start(root=first.root, open_browser=False)
        self.assertEqual(self.goose.call_count, 1)

    def test_concurrent_starts_select_one_session_and_launch_once(self):
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
            results = list(executor.map(lambda _: coding_demo.start_session(open_browser=False), range(4)))
        self.handles.extend(results)
        self.assertEqual(len({(r.root, r.url) for r in results}), 1)
        self.assertEqual(self.goose.call_count, 1)
        self.assertEqual(len(list((self.repo / 'demo-runs').glob('coding-*'))), 1)

    def test_pointer_commit_failure_preserves_previous_selection_and_staged_root(self):
        first = self.start(open_browser=False)
        pointer = self.pointer()
        replace = os.replace
        def fail_pointer(source, destination, **kwargs):
            if destination == 'current.json':
                raise OSError('pointer publication failed')
            return replace(source, destination, **kwargs)
        with patch.object(coding_demo.os, 'replace', side_effect=fail_pointer):
            with self.assertRaises(OSError):
                self.start(new_session=True, open_browser=False)
        self.assertEqual(self.pointer(), pointer)
        self.assertEqual(self.request(first.url, 'state')[0], 200)
        roots = list((self.repo / 'demo-runs').glob('coding-*'))
        self.assertEqual(len(roots), 2)
        self.assertTrue(all((r / 'operator/state.json').exists() for r in roots))
        self.assertEqual(self.goose.call_count, 1)

    def test_ui_failure_commits_session_and_retry_never_opens_new_recipe(self):
        self.goose.side_effect = OSError('Goose unavailable')
        with patch('builtins.print') as output:
            first = self.start(open_browser=False)
            resumed = self.start(open_browser=False)
        self.assertEqual(first.url, resumed.url)
        self.assertEqual(json.loads(self.pointer()), {'root': first.root.name})
        self.assertEqual(self.goose.call_count, 1)
        self.assertEqual(self.request(first.url, 'state')[0], 200)
        messages = '\n'.join(str(call) for call in output.call_args_list)
        self.assertIn(str(first.root / 'desktop-recipe.json'), messages)
        self.assertIn(first.url, messages)

    def test_foreign_listener_preserves_saved_url_and_current_pointer(self):
        first = self.start(open_browser=False)
        first.close()
        pointer = self.pointer()
        metadata = (first.root / 'inspector.json').read_bytes()
        port = urlsplit(first.url).port
        listener = socket.socket()
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(('127.0.0.1', port))
        listener.listen()
        try:
            with self.assertRaises(OSError):
                self.start(open_browser=False)
        finally:
            listener.close()
        self.assertEqual(self.pointer(), pointer)
        self.assertEqual((first.root / 'inspector.json').read_bytes(), metadata)
        self.assertEqual(self.goose.call_count, 1)

    def test_malformed_urls_fail_before_network_or_publication(self):
        first = self.start(open_browser=False)
        pointer = self.pointer()
        for url in ['https://127.0.0.1:1234/token/', 'http://localhost:1234/token/',
                    'http://127.0.0.1:1234/token/?query=1', 'http://127.0.0.1:1234/../',
                    'http://user@127.0.0.1:1234/token/', 'http://127.0.0.1:1234/token/#x']:
            with self.subTest(url=url):
                coding_demo.write_metadata(first.root, 'inspector.json', {'url': url, 'pid': os.getpid()})
                metadata = (first.root / 'inspector.json').read_bytes()
                with patch.object(coding_demo.http.client, 'HTTPConnection') as connection:
                    with self.assertRaises(ValueError):
                        self.start(open_browser=False)
                    connection.assert_not_called()
                self.assertEqual(self.pointer(), pointer)
                self.assertEqual((first.root / 'inspector.json').read_bytes(), metadata)

    def test_malformed_display_mode_is_unavailable_and_preserves_selection(self):
        first = self.start(open_browser=False)
        pointer = self.pointer()
        coding_demo.write_metadata(first.root, 'inspector.json', {'url': first.url, 'view_mode': []})
        metadata = (first.root / 'inspector.json').read_bytes()
        with self.assertRaises(ValueError):
            self.start(open_browser=False)
        self.assertEqual(self.request(first.url, 'state')[0], 503)
        self.assertEqual(self.request(first.url, 'identity')[0], 200)
        self.assertEqual(self.pointer(), pointer)
        self.assertEqual((first.root / 'inspector.json').read_bytes(), metadata)

    def test_invalid_pointer_paths_do_not_create_fresh_session(self):
        first = self.start(open_browser=False)
        for value in ['../escape', str(first.root), 'nested/session', '.', '']:
            with self.subTest(value=value):
                pointer = self.repo / 'demo-runs/current.json'
                pointer.write_text(json.dumps({'root': value}))
                before = pointer.read_bytes()
                with self.assertRaises(ValueError):
                    self.start(open_browser=False)
                self.assertEqual(pointer.read_bytes(), before)
                self.assertEqual(self.goose.call_count, 1)

    def test_unsafe_pointer_and_metadata_are_rejected_without_touching_targets(self):
        first = self.start(open_browser=False)
        for path in [self.repo / 'demo-runs/current.json', first.root / 'inspector.json']:
            original = path.read_bytes()
            for kind in ['symlink', 'hardlink', 'directory']:
                with self.subTest(path=path.name, kind=kind):
                    path.unlink()
                    target = self.repo / 'unrelated.json'
                    target.write_bytes(original)
                    if kind == 'symlink':
                        path.symlink_to(target)
                    elif kind == 'hardlink':
                        os.link(target, path)
                    else:
                        path.mkdir()
                    with self.assertRaises(OSError):
                        self.start(open_browser=False)
                    self.assertEqual(target.read_bytes(), original)
                    path.rmdir() if kind == 'directory' else path.unlink()
                    path.write_bytes(original)

    def test_inspector_publication_failure_preserves_previous_pointer(self):
        first = self.start(open_browser=False)
        pointer = self.pointer()
        replace = os.replace
        def fail_metadata(source, destination, **kwargs):
            if destination == 'inspector.json':
                raise OSError('metadata publication failed')
            return replace(source, destination, **kwargs)
        with patch.object(coding_demo.os, 'replace', side_effect=fail_metadata):
            with self.assertRaises(OSError):
                self.start(new_session=True, open_browser=False)
        self.assertEqual(self.pointer(), pointer)
        self.assertEqual(self.request(first.url, 'state')[0], 200)
        self.assertEqual(self.goose.call_count, 1)

    def test_server_thread_failure_preserves_previous_pointer(self):
        first = self.start(open_browser=False)
        pointer = self.pointer()
        with patch.object(coding_demo.threading.Thread, 'start', side_effect=RuntimeError('thread failed')):
            with self.assertRaises(RuntimeError):
                self.start(new_session=True, open_browser=False)
        self.assertEqual(self.pointer(), pointer)
        self.assertEqual(self.goose.call_count, 1)

    def test_temp_file_sync_failure_preserves_metadata_and_pointer(self):
        first = self.start(open_browser=False)
        first.close()
        pointer = self.pointer()
        metadata = (first.root / 'inspector.json').read_bytes()
        with patch.object(coding_demo.os, 'fsync', side_effect=OSError('file sync failed')):
            with self.assertRaises(OSError):
                self.start(open_browser=False)
        self.assertEqual(self.pointer(), pointer)
        self.assertEqual((first.root / 'inspector.json').read_bytes(), metadata)
        self.assertEqual(list(first.root.glob('.inspector.json-*')), [])
        self.assertEqual(self.goose.call_count, 1)

    def test_recipe_sync_failure_leaves_old_selection_and_staged_root(self):
        first = self.start(open_browser=False)
        pointer = self.pointer()
        fsync = os.fsync
        def fail_recipe_sync(descriptor):
            info = os.fstat(descriptor)
            for path in (self.repo / 'demo-runs').glob('coding-*/desktop-recipe.json'):
                if path.parent.resolve() != first.root and path.stat().st_ino == info.st_ino:
                    raise OSError('recipe sync failed')
            return fsync(descriptor)
        with patch.object(coding_demo.os, 'fsync', side_effect=fail_recipe_sync):
            with self.assertRaises(OSError):
                self.start(new_session=True, open_browser=False)
        self.assertEqual(self.pointer(), pointer)
        self.assertEqual(self.goose.call_count, 1)
        staged = next(r for r in (self.repo / 'demo-runs').glob('coding-*') if r.resolve() != first.root)
        self.assertTrue((staged / 'operator/state.json').exists())
        self.assertFalse((staged / 'inspector.json').exists())

    def test_directory_sync_failure_warns_after_selection_is_committed(self):
        first = self.start(open_browser=False)
        fsync = os.fsync
        def fail_directory_sync(descriptor):
            if stat.S_ISDIR(os.fstat(descriptor).st_mode):
                raise OSError('directory sync failed')
            return fsync(descriptor)
        with patch.object(coding_demo.os, 'fsync', side_effect=fail_directory_sync), patch('builtins.print') as output:
            second = self.start(new_session=True, open_browser=False)
        self.assertNotEqual(first.root, second.root)
        self.assertEqual(json.loads(self.pointer()), {'root': second.root.name})
        self.assertEqual(self.request(second.url, 'state')[0], 200)
        self.assertEqual(self.start(open_browser=False).root, second.root)
        messages = '\n'.join(str(call) for call in output.call_args_list)
        self.assertIn('was published, but directory sync failed', messages)
        self.assertIn('power-loss durability is uncertain', messages)

    def test_view_and_rehearsal_creation_do_not_select_current(self):
        first = self.start(open_browser=False)
        pointer = self.pointer()
        other = coding_demo.fresh_session()
        with patch.object(coding_demo.Inspector, 'wait', autospec=True, side_effect=lambda result: result.close()):
            coding_demo.serve(other, open_browser=False, recorded=True)
        self.assertEqual(self.pointer(), pointer)
        self.assertTrue((other / 'inspector.json').exists())
        self.assertEqual(self.goose.call_count, 1)

    def test_view_then_start_updates_mode_on_the_same_live_server(self):
        first = self.start(open_browser=False)
        pointer = self.pointer()
        self.assertEqual(self.request(first.url, 'state')[1]['view_mode'], 'live')
        coding_demo.serve(first.root, open_browser=False, recorded=True)
        self.assertEqual(self.request(first.url, 'state')[1]['view_mode'], 'recorded')
        self.assertEqual(self.pointer(), pointer)
        resumed = self.start(open_browser=False)
        self.assertEqual(resumed.url, first.url)
        self.assertEqual(self.request(first.url, 'state')[1]['view_mode'], 'live')
        self.assertEqual(self.goose.call_count, 1)

    def test_adoption_of_a_recorded_inspector_switches_to_live_at_saved_url(self):
        self.start(open_browser=False)
        other = coding_demo.fresh_session()
        with patch.object(coding_demo.Inspector, 'wait', autospec=True, side_effect=lambda result: result.close()):
            coding_demo.serve(other, open_browser=False, recorded=True)
        saved = json.loads((other / 'inspector.json').read_text())['url']
        adopted = self.start(root=other, open_browser=False)
        self.assertEqual(adopted.url, saved)
        self.assertEqual(self.request(saved, 'state')[1]['view_mode'], 'live')
        self.assertEqual(self.goose.call_count, 1)

    def test_explicit_adoption_rejects_symlink_root(self):
        first = self.start(open_browser=False)
        pointer = self.pointer()
        link = self.repo / 'demo-runs/linked-session'
        link.symlink_to(first.root, target_is_directory=True)
        with self.assertRaises(OSError):
            self.start(root=link, open_browser=False)
        self.assertEqual(self.pointer(), pointer)

    def test_resuming_does_not_require_goose_binary(self):
        first = self.start(open_browser=False)
        with patch.object(coding_demo.legacy, 'verify_binary', side_effect=OSError('Goose missing')):
            self.assertEqual(self.start(open_browser=False).url, first.url)
            first.close()
            self.assertEqual(self.start(open_browser=False).url, first.url)


if __name__ == '__main__':
    unittest.main()

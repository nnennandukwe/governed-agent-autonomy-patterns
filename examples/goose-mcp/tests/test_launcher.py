"""Operator-facing regression tests; no provider calls or credentials required."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
from unittest import mock
import tempfile
import unittest

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
import install_goose

spec = importlib.util.spec_from_file_location("demo", HERE / "demo.py")
demo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(demo)


class LauncherTests(unittest.TestCase):
    def test_engine_default_is_compact_and_json_remains_machine_readable(self):
        """The presenter view stays compact while full JSON remains parseable."""
        normal = subprocess.run(["python3", str(HERE / "demo.py"), "engine"], capture_output=True, text=True, check=True)
        self.assertIn("workflow.completion_authorized", normal.stdout)
        self.assertIn("Receipt verified: True", normal.stdout)
        self.assertLess(len(normal.stdout.splitlines()), 30)
        raw = subprocess.run(["python3", str(HERE / "demo.py"), "engine", "--json"], capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(raw.stdout)["receipt"]["body"]["terminal_status"], "completed")

    def test_status_reports_symlink_refusal_without_reading_its_destination(self):
        """Status uses the checked Rust observation and never follows a rejected symlink."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve() / "demo"
            demo.binary("init", root, capture=True)
            external = Path(directory) / "external.txt"
            external.write_text("DO NOT READ THIS THROUGH STATUS")
            target = root / "workspace" / "release.json"
            target.unlink()
            target.symlink_to(external)
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                demo.summary(root)
            self.assertIn("observation unavailable", output.getvalue())
            self.assertNotIn(external.read_text(), output.getvalue())

    def test_unknown_request_approval_fails_without_authority(self):
        """An unknown ID produces a nonzero exit and leaves approval state empty."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve() / "demo"
            demo.binary("init", root, capture=True)
            result = subprocess.run(["python3", str(HERE / "demo.py"), "approve", str(root), "unknown", "--yes"], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Unknown request_id",result.stderr)
            self.assertEqual(demo.binary("inspect",root / "operator",capture=True)["proposals"],{})


    def test_timeout_with_zero_exit_is_failure_and_redacts_the_prompt(self):
        """A simulated timeout cannot pass as a real run or persist the dummy key."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "workspace").mkdir()
            process = mock.Mock(pid=1234, returncode=0)
            process.communicate.side_effect = [subprocess.TimeoutExpired("simulated", 1), ("stopped dummy-unit-key", None)]
            with mock.patch.object(demo, "session", return_value=root), mock.patch.object(demo, "verify_binary"), mock.patch.object(demo, "key_from_file", return_value="dummy-unit-key"), mock.patch.object(demo.subprocess, "Popen", return_value=process), mock.patch.object(demo.os, "killpg"), contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaises(SystemExit):
                    demo.goose(root, "do not retain dummy-unit-key", timeout=1, provider="openai")
            transcript = next((root / "transcripts").glob("*.txt")).read_text()
            self.assertIn("OPERATOR TIMEOUT", transcript)
            self.assertNotIn("dummy-unit-key", transcript)
            self.assertIn("[REDACTED]", transcript)

    def test_interrupted_process_group_escalates_to_kill(self):
        """A simulated interrupt reaps a process group that ignores termination."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "workspace").mkdir()
            process = mock.Mock(pid=1234, returncode=-9)
            process.communicate.side_effect = [KeyboardInterrupt(), subprocess.TimeoutExpired("simulated", 5), ("stopped", None)]
            with mock.patch.object(demo, "session", return_value=root), mock.patch.object(demo, "verify_binary"), mock.patch.object(demo, "key_from_file", return_value="dummy-unit-key"), mock.patch.object(demo.subprocess, "Popen", return_value=process), mock.patch.object(demo.os, "killpg") as signals, contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaises(SystemExit):
                    demo.goose(root, "simulated interruption", provider="openai")
                self.assertEqual(signals.call_args_list, [mock.call(1234,demo.signal.SIGTERM),mock.call(1234,demo.signal.SIGKILL)])
            self.assertIn("OPERATOR INTERRUPTED",next((root / "transcripts").glob("*.txt")).read_text())

    def test_operator_rendering_cannot_execute_controls_or_hide_bidi_text(self):
        """Control bytes remain visible and cannot alter the operator's terminal display."""
        payload = "before\n\x1b[2J\rAPPROVE\u202eafter\tend\\x1b"
        rendered = demo.terminal_text(payload)
        self.assertNotIn("\x1b", rendered)
        self.assertNotIn("\r", rendered)
        self.assertNotIn("\u202e", rendered)
        self.assertNotIn("\t", rendered)
        self.assertIn(r"\x1b[2J", rendered)
        self.assertIn(r"\u202e", rendered)
        self.assertIn(r"\\x1b", rendered)
        self.assertIn("before\n", rendered)

    def test_codex_auth_is_temporary_and_does_not_copy_refresh_authority(self):
        """Only short-lived access reaches the isolated runtime, including on failure."""
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            source = home / ".config/goose/chatgpt_codex/tokens.json"
            source.parent.mkdir(parents=True)
            data = {"access_token": "dummy-access", "account_id": "dummy-account",
                    "refresh_token": "dummy-refresh", "id_token": "dummy-identity",
                    "expires_at": "2099-01-01T00:00:00Z"}
            source.write_text(json.dumps(data))
            source.chmod(0o600)
            before = source.read_bytes()
            with mock.patch.object(demo.Path, "home", return_value=home):
                with self.assertRaisesRegex(RuntimeError, "simulated launch failure"):
                    with demo.provider_environment(home, "chatgpt_codex", "gpt-5.5", 90) as (env, secrets):
                        cache_root = Path(env["GOOSE_PATH_ROOT"])
                        cache = cache_root / "config/chatgpt_codex/tokens.json"
                        copied = json.loads(cache.read_text())
                        self.assertEqual(copied["refresh_token"], "")
                        self.assertIsNone(copied["id_token"])
                        self.assertEqual(copied["access_token"], data["access_token"])
                        self.assertEqual(cache.stat().st_mode & 0o777, 0o600)
                        self.assertEqual(cache_root.stat().st_mode & 0o777, 0o700)
                        self.assertNotIn("OPENAI_API_KEY", env)
                        self.assertNotIn("dummy-access", json.dumps(env))
                        self.assertIn("dummy-access", secrets)
                        raise RuntimeError("simulated launch failure")
            self.assertFalse(cache_root.exists())
            self.assertEqual(source.read_bytes(), before)

    def test_expiring_codex_auth_stops_before_goose_can_refresh_it(self):
        """Expired access is rejected locally without rotating desktop credentials."""
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            source = home / ".config/goose/chatgpt_codex/tokens.json"
            source.parent.mkdir(parents=True)
            source.write_text(json.dumps({"access_token": "dummy", "account_id": "dummy",
                                          "expires_at": "2000-01-01T00:00:00Z"}))
            source.chmod(0o600)
            with mock.patch.object(demo.Path, "home", return_value=home):
                with self.assertRaisesRegex(SystemExit, "expires too soon"):
                    demo.codex_token(90)

    def test_codex_auth_refuses_symlinks_and_shared_credentials(self):
        """The launcher cannot read a redirected or shared sign-in file."""
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            source = home / ".config/goose/chatgpt_codex/tokens.json"
            source.parent.mkdir(parents=True)
            external = home / "external"
            external.write_text("must not be read")
            source.symlink_to(external)
            with mock.patch.object(demo.Path, "home", return_value=home):
                with self.assertRaisesRegex(SystemExit, "symlink"):
                    demo.codex_token(90)
                source.unlink()
                source.write_text("must not be read")
                source.chmod(0o644)
                with self.assertRaisesRegex(SystemExit, "owner-only"):
                    demo.codex_token(90)

    def test_codex_auth_rejects_a_redirected_config_ancestor_before_reading(self):
        """A symlink at .config cannot redirect the credential lookup."""
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            external = home / "elsewhere"
            (external / "goose/chatgpt_codex").mkdir(parents=True)
            (home / ".config").symlink_to(external, target_is_directory=True)
            with mock.patch.object(demo.Path, "home", return_value=home), mock.patch.object(demo.os, "open") as opened:
                with self.assertRaisesRegex(SystemExit, "must not be a symlink"):
                    demo.codex_token(90)
                opened.assert_not_called()

    def test_shared_session_directories_are_rejected_before_storing_data(self):
        """Existing permissive directories are refused instead of silently reused."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("goose", "transcripts"):
                path = root / name
                path.mkdir(mode=0o755)
                path.chmod(0o755)
                with self.subTest(name=name), self.assertRaisesRegex(SystemExit, "owner-only"):
                    demo.private_directory(path, create=True)
                self.assertEqual(list(path.iterdir()), [])
            root.chmod(0o755)
            with mock.patch.object(demo, "binary") as binary:
                with self.assertRaisesRegex(SystemExit, "owner-only"):
                    demo.session(root)
                binary.assert_not_called()
            root.chmod(0o700)
            with mock.patch.object(demo.os, "getuid", return_value=-1):
                with self.assertRaisesRegex(SystemExit, "owner-only"):
                    demo.private_directory(root)

    def test_api_key_setup_and_read_refuse_a_redirected_config_ancestor(self):
        """The optional API path rejects redirection before prompting or writing."""
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            external = home / "elsewhere"
            external.mkdir()
            (home / ".config").symlink_to(external, target_is_directory=True)
            with mock.patch.object(demo.Path, "home", return_value=home), mock.patch.object(demo.getpass, "getpass") as prompt:
                for action in (demo.configure, demo.key_from_file):
                    with self.subTest(action=action.__name__), self.assertRaisesRegex(SystemExit, "symlink"):
                        action()
                prompt.assert_not_called()
            self.assertEqual(list(external.iterdir()), [])

    def test_untrusted_existing_binary_is_rejected_by_content(self):
        """A fake executable cannot gain trust from its own version output."""
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "goose"
            path.write_text("#!/bin/sh\necho 1.50.0\n")
            path.chmod(0o700)
            with self.assertRaisesRegex(SystemExit, "checksum mismatch"):
                install_goose.verify_binary(path)

    def test_failed_install_does_not_publish_a_partial_binary(self):
        """A simulated pre-publication sync failure leaves no executable destination."""
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            with mock.patch.object(install_goose.os, "fsync", side_effect=OSError("injected sync failure")):
                with self.assertRaises(OSError):
                    install_goose.publish_binary(path, b"complete fixture bytes")
            self.assertFalse((path / "goose").exists())
            self.assertEqual(list(path.iterdir()), [])

    def test_install_publication_never_replaces_an_existing_file(self):
        """Publication preserves another completed installation if the name exists."""
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / "goose").write_bytes(b"existing")
            with self.assertRaises(FileExistsError):
                install_goose.publish_binary(path,b"replacement")
            self.assertEqual((path / "goose").read_bytes(),b"existing")


if __name__ == "__main__":
    unittest.main()

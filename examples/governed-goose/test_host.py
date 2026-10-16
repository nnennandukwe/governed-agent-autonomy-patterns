"""Exercise timeout cleanup through the runner's actual owned process boundary."""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from server import capture_sandbox, sandbox_operation

from run import run_host, sandbox_observations


class HostCleanupTests(unittest.TestCase):
    """The controlled group must stop even when its host can't run ACP cleanup."""

    def test_timeout_stops_the_hosts_live_child(self):
        """Force a parent timeout while a real descendant holds its pipes open."""
        with tempfile.TemporaryDirectory(prefix="goose-host-timeout-") as temporary:
            root = pathlib.Path(temporary)
            script = root / "host.py"
            child_pid = root / "child.pid"
            script.write_text(
                "import pathlib, subprocess, sys, time\n"
                "child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])\n"
                "pathlib.Path(sys.argv[1]).write_text(str(child.pid))\n"
                "time.sleep(30)\n"
            )
            with self.assertRaises(subprocess.TimeoutExpired):
                run_host([sys.executable, str(script), str(child_pid)], timeout=0.5)
            self.assertTrue(child_pid.exists(), "The live-child fixture didn't reach its fault boundary")
            result = subprocess.run(["ps", "-p", child_pid.read_text(), "-o", "stat="], capture_output=True, text=True, check=False)
            state = result.stdout.strip()
            self.assertTrue(not state or state.startswith("Z"), "A live child survived the host timeout")

    def test_sandbox_response_survives_the_previous_transport_cutoff(self):
        """Retain a real child response after eight seconds under its adopted budget."""
        with tempfile.TemporaryDirectory(prefix="goose-sandbox-response-") as temporary:
            root = pathlib.Path(temporary)
            child = root / "sandbox.py"
            child.write_text(
                "import json, sys, time\n"
                "json.load(sys.stdin)\n"
                "time.sleep(8.2)\n"
                "print(json.dumps({'status':'exited','exit_code':0,'cleanup':'removed','publication_error':None}))\n"
            )
            context = root / "context.json"
            observation = root / "observation.json"
            arguments = {"operation": "mcp_echo", "content": "retained"}
            context.write_text(json.dumps({
                "arguments": arguments, "executable": sys.executable,
                "config": str(child), "observation": str(observation),
                "transport": str(root / "transport.json"), "request_digest": "sha256:" + "0" * 64,
                "request": {"limits": {"wall_clock_ms": 10000, "cleanup_ms": 2000}},
            }))
            with patch.dict(os.environ, {"GAAP_SANDBOX_CONTEXT": str(context)}):
                result = sandbox_operation(arguments)
            self.assertFalse(result["isError"])
            self.assertEqual(json.loads(observation.read_bytes())["status"], "exited")
            self.assertEqual(observation.stat().st_mode & 0o777, 0o600)

    def test_failed_adapter_transport_retains_original_bytes(self):
        """Invalid output and nonzero exits remain available before parsing fails."""
        for program, expected_error in (
            ("import sys;sys.stdout.write('partial output');sys.stderr.write('runtime detail');sys.exit(7)", ValueError),
            ("print('invalid observation')", json.JSONDecodeError),
        ):
            with self.subTest(program=program), tempfile.TemporaryDirectory(prefix="sandbox-failure-") as temporary:
                root = pathlib.Path(temporary)
                child = root / "child.py"
                child.write_text(program)
                context = root / "context.json"
                transport = root / "transport.json"
                observation = root / "observation.json"
                context.write_text(json.dumps({
                    "arguments": {}, "request": {"limits": {"wall_clock_ms": 100, "cleanup_ms": 100}},
                    "request_digest": "sha256:" + "0" * 64,
                    "executable": sys.executable, "config": str(child),
                    "transport": str(transport), "observation": str(observation),
                }))
                with patch.dict(os.environ, {"GAAP_SANDBOX_CONTEXT": str(context)}), self.assertRaises(expected_error):
                    sandbox_operation({})
                retained = json.loads(transport.read_bytes())
                self.assertTrue(retained['stdout'])
                self.assertEqual(transport.stat().st_mode & 0o777, 0o600)
                self.assertFalse(observation.exists())
                if retained['exit_code'] == 7:
                    self.assertEqual(bytes(retained['stdout']), b'partial output')
                    self.assertEqual(bytes(retained['stderr']), b'runtime detail')

    def test_adapter_pipe_overflow_retains_a_bounded_prefix(self):
        """Output overflow terminates its owned group without unbounded retention."""
        retained = capture_sandbox([sys.executable, '-c', "import os;os.write(1,b'x'*(2*1024*1024))"], b'', 2, 0.1)
        self.assertTrue(retained['stdout_truncated'])
        self.assertEqual(len(retained['stdout']), 1024 * 1024)
        self.assertEqual(retained['termination'], 'output_overflow')

    def test_adapter_deadline_stops_inherited_output_pipes(self):
        """A parent exit can't leave its response reader waiting on a descendant."""
        retained = capture_sandbox([sys.executable, '-c',
            "import subprocess,sys;child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)']);print(child.pid,flush=True)"], b'', 0.1, 0.1)
        self.assertEqual(retained['termination'], 'response_timeout')
        child_pid = bytes(retained['stdout']).decode().strip()
        result = subprocess.run(['ps', '-p', child_pid, '-o', 'stat='], capture_output=True, text=True, check=False)
        self.assertTrue(not result.stdout.strip() or result.stdout.strip().startswith('Z'))

    def test_raw_and_canonical_observations_must_agree(self):
        """A second encoding can't manufacture an extra execution or hide a conflict."""
        observation = {'schema_version': 'gaap.sandbox-observation/0.1',
                       'request_digest': 'sha256:' + '0' * 64, 'owned_resource': 'owned-first'}
        canonical = list(json.dumps(observation, sort_keys=True).encode())
        raw = list((json.dumps(observation) + '\n').encode())
        record = {'evidence_blobs': {'canonical': canonical, 'transport': raw}}
        self.assertEqual(sandbox_observations(record), [observation])
        record['evidence_blobs']['conflict'] = list(json.dumps({**observation, 'owned_resource': 'owned-second'}).encode())
        with self.assertRaisesRegex(AssertionError, 'Conflicting sandbox observations'):
            sandbox_observations(record)

    def test_successful_host_exits_normally(self):
        """Keep a normal host exit as the control for timeout cleanup."""
        output = run_host([sys.executable, "-c", "print('host completed')"], timeout=2)
        self.assertEqual(output, b"host completed\n")


if __name__ == "__main__":
    unittest.main()

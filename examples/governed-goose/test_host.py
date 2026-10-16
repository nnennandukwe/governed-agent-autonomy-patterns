"""Exercise timeout cleanup through the runner's actual owned process boundary."""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from server import sandbox_operation

from run import run_host


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
                "request": {"limits": {"wall_clock_ms": 10000, "cleanup_ms": 2000}},
            }))
            with patch.dict(os.environ, {"GAAP_SANDBOX_CONTEXT": str(context)}):
                result = sandbox_operation(arguments)
            self.assertFalse(result["isError"])
            self.assertEqual(json.loads(observation.read_bytes())["status"], "exited")
            self.assertEqual(observation.stat().st_mode & 0o777, 0o600)

    def test_successful_host_exits_normally(self):
        """Keep a normal host exit as the control for timeout cleanup."""
        output = run_host([sys.executable, "-c", "print('host completed')"], timeout=2)
        self.assertEqual(output, b"host completed\n")


if __name__ == "__main__":
    unittest.main()

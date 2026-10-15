"""Exercise timeout cleanup through the runner's actual owned process boundary."""
import pathlib
import subprocess
import sys
import tempfile
import unittest

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

    def test_successful_host_exits_normally(self):
        """Keep a normal host exit as the control for timeout cleanup."""
        run_host([sys.executable, "-c", "print('host completed')"], timeout=2)


if __name__ == "__main__":
    unittest.main()

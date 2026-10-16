"""Exercise process-tree timeouts in the mutation runner."""
import importlib.util
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("mutant_runner", Path(__file__).with_name("test-state-machine-mutants.py"))
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)


@unittest.skipUnless(os.name == "posix", "mutation process groups require POSIX")
class MutationDeadlineTests(unittest.TestCase):
    """A descendant holding output pipes mustn't defeat command deadlines."""

    def test_descendant_pipes_are_closed_after_the_group_is_stopped(self):
        """Stop a pipe-holding child even after its launcher exits."""
        command = [sys.executable, "-c", "import subprocess,sys; subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)']); print('ready', flush=True)"]
        with tempfile.TemporaryDirectory(prefix="gaap-mutant-deadline-") as directory:
            started = time.monotonic()
            with self.assertRaisesRegex(SystemExit, "process group was stopped"):
                RUNNER.run_cargo(command, Path(directory), os.environ.copy(), started + 0.5)
            self.assertLess(time.monotonic() - started, 5)

    def test_prior_ci_steps_reduce_the_remaining_budget(self):
        """Carry the CI deadline into the runner instead of starting a fresh budget."""
        with patch.dict(os.environ, {"GAAP_MUTANT_DEADLINE_UNIX": "1050"}), patch.object(RUNNER.time, "time", return_value=1000), patch.object(RUNNER.time, "monotonic", return_value=100):
            self.assertEqual(RUNNER.suite_deadline(), 150)

    def test_exhausted_suite_doesnt_start_another_command(self):
        """Reject further work when the aggregate deadline has passed."""
        with self.assertRaisesRegex(SystemExit, "budget is exhausted"):
            RUNNER.run_cargo(["this-executable-must-never-start"], Path.cwd(), os.environ.copy(), time.monotonic() - 1)


if __name__ == "__main__":
    unittest.main()

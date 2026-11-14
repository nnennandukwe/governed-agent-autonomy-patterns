"""Exercise diagnostic cleanup through a synthetic Docker CLI process."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("diagnose-native-oom.py")
FAKE_DOCKER = r'''
import json, os, sys, time
from pathlib import Path
arguments = sys.argv[1:]
state_path = Path(os.environ["FAKE_DOCKER_STATE"])
state = json.loads(state_path.read_text())
if arguments[:2] == ["image", "inspect"]:
    print("sha256:fixture")
elif arguments[:2] == ["container", "create"]:
    state["created"] = True
    state["name"] = arguments[arguments.index("--name") + 1]
    state["owner"] = arguments[arguments.index("--label") + 1].split("=", 1)[1]
    if state["mode"] == "foreign":
        state["owner"] = "another-owner"
    state_path.write_text(json.dumps(state))
    if state["mode"] == "create_loss":
        print("create response lost", file=sys.stderr)
        raise SystemExit(1)
    print("a" * 64)
elif arguments[:2] == ["container", "start"]:
    raise SystemExit(137)
elif arguments[:2] == ["container", "inspect"]:
    if state["mode"] == "probe":
        print(json.dumps([{"Id": "a"*64, "State": {"OOMKilled": True, "ExitCode": 137},
            "Config": {"Labels": {"org.gaap.owner": state["owner"]}},
            "HostConfig": {"Memory": 33554432, "MemorySwap": 33554432}}]))
    else:
        print("malformed inspection")
elif arguments[:2] == ["container", "ls"]:
    expected_owner = next(value.split("=", 2)[2] for value in arguments if value.startswith("label=org.gaap.owner="))
    if state["created"] and state["owner"] == expected_owner:
        print("a" * 64)
elif arguments[:1] == ["events"]:
    print(json.dumps({"Action": "oom", "Actor": {"ID": "a"*64}}), flush=True)
    if "--until" not in arguments:
        time.sleep(2)
elif arguments[:2] == ["container", "rm"]:
    state["created"] = False
    state["removed"] = arguments[-1]
    state_path.write_text(json.dumps(state))
else:
    raise SystemExit("unexpected fake Docker operation: " + repr(arguments))
'''


class DiagnosticCleanupTests(unittest.TestCase):
    """Retain ownership checks when inspection output can't be decoded."""

    def exercise(self, mode):
        """Run the real diagnostic entry point against isolated CLI fixtures."""
        with tempfile.TemporaryDirectory(prefix="gaap-19-cli-") as directory:
            root = Path(directory)
            state_path = root / "state.json"
            state_path.write_text(json.dumps({"mode": mode, "created": False}))
            docker = root / "docker"
            docker.write_text("#!" + sys.executable + "\n" + FAKE_DOCKER)
            docker.chmod(0o700)
            sudo = root / "sudo"
            sudo.write_text("#!/bin/sh\nexit 0\n")
            sudo.chmod(0o700)
            environment = dict(os.environ, PATH=str(root) + os.pathsep + os.environ["PATH"],
                GITHUB_ACTIONS="true", DOCKER_HOST="unix:///var/run/docker.sock",
                FAKE_DOCKER_STATE=str(state_path), PYTHONDONTWRITEBYTECODE="1")
            result = subprocess.run([sys.executable, "-B", str(SCRIPT), "--run"],
                env=environment, capture_output=True, text=True, timeout=8)
            return json.loads(state_path.read_text()), result

    def test_malformed_inspection_doesnt_leave_the_owned_fixture(self):
        """A failed state probe still removes its own exact container."""
        state, result = self.exercise("normal")
        self.assertFalse(state["created"], result.stderr + result.stdout)
        self.assertEqual(state["removed"], "a" * 64)

    def test_lost_create_response_still_cleans_the_owned_fixture(self):
        """A daemon-side create can succeed after its client reports failure."""
        state, result = self.exercise("create_loss")
        self.assertFalse(state["created"], result.stderr + result.stdout)
        self.assertEqual(state["removed"], "a" * 64)

    def test_live_probe_retains_output_and_cleanup_completes(self):
        """A live-stream timeout retains complete records and releases its fixture."""
        state, result = self.exercise("probe")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertFalse(state["created"])
        record = next(line.split(" ", 2)[2] for line in result.stdout.splitlines()
            if line.startswith("[DEBUG-gaap19-oom] live_events "))
        value = json.loads(record)
        self.assertTrue(value["timed_out"])
        self.assertIn("oom", value["stdout"])

    def test_foreign_owner_is_preserved(self):
        """A mismatched owner prevents fixture deletion."""
        state, result = self.exercise("foreign")
        self.assertTrue(state["created"], result.stderr + result.stdout)
        self.assertNotIn("removed", state)


if __name__ == "__main__":
    unittest.main()

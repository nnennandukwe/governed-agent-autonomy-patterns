#!/usr/bin/env python3
"""Temporary CI probe for the observed native OOM evidence failure."""
import argparse
import json
import os
import re
import subprocess
import time
import uuid

PREFIX = "[DEBUG-gaap19-oom]"
IMAGE = "python@sha256:2d9aefe2fef018a7eb2c13064c89c71929800fd2e5dccdbf52ea5da5bb8d929a"


def emit(label, value):
    """Print one tagged JSON diagnostic record."""
    print(PREFIX, label, json.dumps(value, sort_keys=True), flush=True)


def command(arguments, timeout=10):
    """Run a trusted CLI command with a deadline and retained-output bounds."""
    result = subprocess.run(arguments, capture_output=True, text=True, timeout=timeout)
    return {"code": result.returncode, "stdout": result.stdout[-65536:], "stderr": result.stderr[-8192:]}


def inspect(name):
    """Read a fixture container's identity, terminal state and memory limits."""
    result = command(["docker", "container", "inspect", name])
    if result["code"] == 0:
        values = json.loads(result["stdout"])
        return {"Id": values[0]["Id"], "State": values[0]["State"],
                "Owner": values[0]["Config"]["Labels"].get("org.gaap.owner"),
                "Memory": values[0]["HostConfig"]["Memory"],
                "MemorySwap": values[0]["HostConfig"]["MemorySwap"]}
    return result



def cleanup(name, owner, container_id):
    """Remove only an exact name/owner match within an independent deadline."""
    deadline = time.monotonic() + 8
    try:
        listing = command(["docker", "container", "ls", "--all", "--no-trunc",
            "--filter", "name=^/"+name+"$", "--filter", "label=org.gaap.owner="+owner,
            "--format", "{{.ID}}"], timeout=3)
        candidates = listing["stdout"].split()
        if listing["code"] != 0:
            emit("cleanup_unconfirmed", listing)
            return
        if not candidates:
            emit("cleanup", "no_matching_owned_resource")
            return
        if len(candidates) != 1 or not re.fullmatch(r"[a-f0-9]{64}", candidates[0]):
            emit("cleanup_unconfirmed", "unexpected owner-filtered container identity")
            return
        observed_id = candidates[0]
        if container_id is not None and observed_id != container_id:
            emit("cleanup_unconfirmed", "container identity changed")
            return
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            emit("cleanup_unconfirmed", "cleanup deadline elapsed")
            return
        result = command(["docker", "container", "rm", "--force", "--volumes", observed_id], timeout=min(5, remaining))
        emit("cleanup" if result["code"] == 0 else "cleanup_unconfirmed", result)
    except (OSError, subprocess.TimeoutExpired, ValueError) as exception:
        emit("cleanup_unconfirmed", str(exception))

def main():
    """Collect one explicitly enabled CI fixture and its OOM evidence."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", action="store_true", required=True,
                        help="Run the bounded fixture against the explicitly selected CI daemon")
    parser.parse_args()
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("DOCKER_HOST") != "unix:///var/run/docker.sock":
        raise SystemExit("Use only the explicit GitHub Actions test daemon")
    for label, arguments in [
        ("kernel_oom", ["sudo", "dmesg", "--ctime"]),
        ("oomd", ["sudo", "journalctl", "-u", "systemd-oomd", "--since", "-5 minutes", "--no-pager"]),
    ]:
        result = command(arguments)
        if label == "kernel_oom":
            result["stdout"] = "\n".join(line for line in result["stdout"].splitlines()
                if any(term in line.lower() for term in ["oom", "out of memory", "killed process"]))[-32768:]
        emit(label, result)
    image = command(["docker", "image", "inspect", IMAGE, "--format", "{{.Id}}"])
    emit("image", image)
    if image["code"]:
        return
    owner = uuid.uuid4().hex
    name = "gaap-19-debug-" + owner
    container_id = None
    try:
        created = command(["docker", "container", "create", "--name", name,
            "--label", "org.gaap.owner=" + owner, "--network", "none", "--read-only",
            "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--user", "1000:1000",
            "--pids-limit", "32", "--memory", "33554432", "--memory-swap", "33554432",
            "--cpus", "1", IMAGE, "/usr/local/bin/python3", "-I", "-S", "-B", "-c",
            "allocation=bytearray(256*1024*1024);print(len(allocation))"])
        emit("create", created)
        if created["code"]:
            return
        container_id = created["stdout"].strip()
        start = time.monotonic()
        emit("start", command(["docker", "container", "start", "--attach", name], timeout=15))
        for delay in [0, 0.25, 1.0]:
            if delay:
                time.sleep(delay)
            emit("state", {"elapsed_ms": round((time.monotonic()-start)*1000), "value": inspect(name)})
        # A finite cached-event window avoids terminating a live client before it flushes.
        emit("events", command(["docker", "events", "--since", "1", "--until", str(int(time.time())+1),
            "--filter", "container="+container_id, "--filter", "event=oom", "--format", "{{json .}}"], timeout=5))
    finally:
        cleanup(name, owner, container_id)



if __name__ == "__main__":
    main()

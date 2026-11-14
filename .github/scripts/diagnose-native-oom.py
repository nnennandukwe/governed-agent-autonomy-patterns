#!/usr/bin/env python3
"""Temporary CI probe for the observed native OOM evidence failure."""
import argparse
import json
import os
import subprocess
import time
import uuid

PREFIX = "[DEBUG-gaap19-oom]"
IMAGE = "python@sha256:2d9aefe2fef018a7eb2c13064c89c71929800fd2e5dccdbf52ea5da5bb8d929a"


def emit(label, value):
    print(PREFIX, label, json.dumps(value, sort_keys=True), flush=True)


def command(arguments, timeout=10):
    result = subprocess.run(arguments, capture_output=True, text=True, timeout=timeout)
    return {"code": result.returncode, "stdout": result.stdout[-65536:], "stderr": result.stderr[-8192:]}


def inspect(name):
    result = command(["docker", "container", "inspect", name])
    if result["code"] == 0:
        values = json.loads(result["stdout"])
        return {"Id": values[0]["Id"], "State": values[0]["State"],
                "Owner": values[0]["Config"]["Labels"].get("org.gaap.owner"),
                "Memory": values[0]["HostConfig"]["Memory"],
                "MemorySwap": values[0]["HostConfig"]["MemorySwap"]}
    return result


def main():
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
        if container_id:
            current = inspect(name)
            if current.get("Id") == container_id and current.get("Owner") == owner:
                emit("cleanup", command(["docker", "container", "rm", "--force", "--volumes", container_id]))
            else:
                emit("cleanup_unconfirmed", current)


if __name__ == "__main__":
    main()

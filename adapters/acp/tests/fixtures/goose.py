"""Independent pinned-Goose profile fixture; it never executes proposed effects."""
import json
import pathlib
import sys
import time

mode = sys.argv[1]


def receive():
    """Read one client message or exit on EOF."""
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    return json.loads(line)


def response(request, result):
    """Return one JSON-RPC response for the request."""
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)


def update(kind):
    """Emit a passive session metadata update."""
    payload = {"sessionUpdate": kind, "title": "Fixture"}
    print(json.dumps({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "goose-fixture", "update": payload}}), flush=True)


request = receive()
if mode == "delayed":
    time.sleep(0.3)
assert request["params"]["clientCapabilities"].get("fs", {}) in ({}, {"readTextFile": False, "writeTextFile": False})
assert request["params"]["clientCapabilities"].get("terminal", False) is False
response(request, {"protocolVersion": 1, "agentCapabilities": {}, "agentInfo": {"name": "goose", "version": "1.54.0"}})
request = receive()
extensions = request["params"]["_meta"]["enabledExtensions"]
assert len(extensions) == 1
assert extensions[0]["type"] == "mcp"
response(request, {"sessionId": "goose-fixture"})
update("session_info_update")
request = receive()
assert request["method"] == "_goose/unstable/session/extensions/list"
inventory = [{"extension": extension, "extensionKey": extension["server"]["name"]} for extension in extensions]
if mode == "extra_extension":
    inventory.append({"extension": {"type": "builtin", "name": "developer"}, "extensionKey": "developer"})
if mode == "changed_extension":
    inventory[0]["extension"]["server"]["args"].append("undeclared")
response(request, {"extensions": inventory})
request = receive()
pathlib.Path("prompt-started").touch()
if mode == "filesystem":
    print(json.dumps({"jsonrpc": "2.0", "id": "write", "method": "fs/write_text_file", "params": {"sessionId": "goose-fixture", "path": str(pathlib.Path.cwd() / "forbidden"), "content": "no"}}), flush=True)
    receive()
response(request, {"stopReason": "end_turn"})

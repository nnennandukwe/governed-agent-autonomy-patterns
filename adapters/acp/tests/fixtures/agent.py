"""Independent ACP v1 wire fixture. No SDK or GAAP code is imported."""
import os
import json
import pathlib
import sys
import time

mode = sys.argv[1]
root = pathlib.Path.cwd()
(root / "agent-pid").write_text(str(os.getpid()))

def send(message):
    """Flush one JSON-RPC message to the client."""
    print(json.dumps(message), flush=True)

def response(request, result):
    """Reply to the exact request identity."""
    send({"jsonrpc": "2.0", "id": request["id"], "result": result})

def receive():
    """Read one client message, exiting when the connection closes."""
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    return json.loads(line)

def update(session, payload):
    """Send a session-scoped progress notification."""
    send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": session, "update": payload}})

request = receive()
if mode == "duplicate_key":
    print('{"jsonrpc":"2.0","id":' + json.dumps(request["id"]) + ',"result":{},"result":{}}', flush=True)
    time.sleep(2)
    sys.exit(0)
if mode == "early_update":
    update("fixture-session", {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "early"}})
if mode == "missing_response":
    time.sleep(30)
    sys.exit(0)
if mode == "partial":
    sys.stdout.write('{"jsonrpc":')
    sys.stdout.flush()
    sys.exit(0)
if mode == "exit":
    sys.exit(2)
if mode == "oversized":
    print("x" * (2 * 1024 * 1024), flush=True)
    time.sleep(2)
if mode == "stderr":
    sys.stderr.write("x" * 100_000)
    sys.stderr.flush()
    time.sleep(2)
version = 2 if mode == "wrong_version" else 1
response(request, {"protocolVersion": version, "agentCapabilities": {"loadSession":"bad"} if mode == "malformed_capabilities" else {}, "agentInfo": {"name": "fixture-agent", "version": "1.0"},
    "authMethods": [{"id":"optional-login","name":"Configure provider"}] if mode == "advertised_auth" else []})
if mode == "duplicate_response":
    response(request, {"protocolVersion": 1, "agentCapabilities": {}, "agentInfo": {"name": "fixture-agent", "version": "1.0"}})
request = receive()
assert request["method"] == "session/new"
assert request["params"]["mcpServers"] == []
if mode == "requires_auth":
    send({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":"Authentication required"}})
    time.sleep(1)
    sys.exit(0)
session = "fixture-session"
response(request, {"sessionId": session})
prompt = receive()
assert prompt["method"] == "session/prompt"
(root / "prompt-started").touch()
if mode == "cancel":
    while True:
        message = receive()
        if message.get("method") == "session/cancel":
            (root / "cancel-received").touch()
            response(prompt, {"stopReason": "cancelled"})
            sys.exit(0)
if mode == "partial_output":
    update(session, {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "SECRET_PARTIAL_TEXT"}})
    sys.exit(0)
if mode == "large_tool_id":
    update(session, {"sessionUpdate": "tool_call", "toolCallId": "x" * 257, "title": "long id", "status": "pending"})
elif mode == "byte_limit":
    for _ in range(40):
        update(session, {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "x" * 512}})
elif mode == "wrong_session":
    update("other-session", {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "wrong"}})
elif mode == "tool_order":
    update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "unknown", "status": "completed"})
elif mode == "terminal":
    send({"jsonrpc": "2.0", "id": "terminal", "method": "terminal/create", "params": {"sessionId": session, "command": "touch", "args": [str(root / "forbidden")]}})
    receive()
elif mode == "permission":
    send({"jsonrpc": "2.0", "id": "permission", "method": "session/request_permission", "params": {
        "sessionId": session, "toolCall": {"toolCallId": "opaque", "title": "Do something"},
        "options": [{"optionId": "yes", "name": "Allow", "kind": "allow_once"}]}})
    answer = receive()
    if answer.get("result", {}).get("outcome", {}).get("outcome") == "selected":
        (root / "forbidden").touch()
elif mode == "report_only":
    update(session, {"sessionUpdate": "tool_call", "toolCallId": "claimed", "title": "Claimed write", "status": "in_progress"})
    update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "claimed", "status": "completed"})
else:
    if mode in ("tool_progress", "progress_timeout"):
        update(session, {"sessionUpdate": "tool_call", "toolCallId": "governed", "title": "Delegated files", "status": "pending"})
        if mode == "tool_progress":
            update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "governed", "status": "in_progress"})
    path = root / "subject.txt"
    if mode == "symlink":
        path = root / "linked.txt"
    if mode == "escape":
        path = root.parent / "outside.txt"
    send({"jsonrpc": "2.0", "id": -7 if mode == "signed_ids" else "write", "method": "fs/write_text_file", "params": {"sessionId": session, "path": str(path), "content": "after\n"}})
    if mode == "progress_timeout":
        deadline = time.monotonic() + 15
        while not (root / "host-started").exists():
            if time.monotonic() > deadline:
                raise RuntimeError("Host dispatch never started")
            time.sleep(0.005)
        update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "governed", "status": "in_progress"})
    if mode == "duplicate_request":
        time.sleep(0.1)
        send({"jsonrpc": "2.0", "id": -7 if mode == "signed_ids" else "write", "method": "fs/write_text_file", "params": {"sessionId": session, "path": str(path), "content": "other\n"}})
    answer = receive()
    if "error" in answer:
        response(prompt, {"stopReason": "refusal"})
        sys.exit(0)
    if mode == "post_effect_exit":
        sys.exit(3)
    if mode == "post_effect_timeout":
        time.sleep(30)
    send({"jsonrpc": "2.0", "id": -8 if mode == "signed_ids" else "read", "method": "fs/read_text_file", "params": {"sessionId": session, "path": str(root / "subject.txt")}})
    answer = receive()
    if mode == "normal":
        assert answer["result"]["content"] == "after\n"
    if mode == "tool_progress":
        update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "governed", "status": "completed"})
    update(session, {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "SECRET_AGENT_TEXT"}})
response(prompt, {"stopReason": "end_turn"})
# Keep the process open until the client closes; all children must be reaped.
while sys.stdin.readline():
    pass

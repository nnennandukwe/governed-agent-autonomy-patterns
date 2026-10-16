"""Proposal ingress and separate, host-approved effect backend for the reference."""
import json
import os
import signal
import pathlib
import socket
import subprocess
import sys

TOOL = {
    "name": "governed_operation",
    "description": "Submit a bounded operation for host authorization.",
    "inputSchema": {
        "type": "object",
        "properties": {
            "operation": {"type": "string", "enum": ["file_write", "terminal_echo", "mcp_echo"]},
            "content": {"type": "string", "maxLength": 8192},
        },
        "required": ["operation", "content"],
        "additionalProperties": False,
    },
}


def operation(arguments):
    """Perform only the host's fixed reference operations after admission."""
    if os.environ.get('GAAP_SANDBOX_CONTEXT'):
        return sandbox_operation(arguments)
    root = pathlib.Path.cwd()
    content = arguments["content"]
    if not isinstance(content, str) or len(content) > 8192 or set(arguments) != {"operation", "content"}:
        raise ValueError("Invalid operation input")
    kind = arguments["operation"]
    if kind == "file_write":
        target = root / "shipping.py"
        if target.is_symlink():
            raise ValueError("Symlink target refused")
        target.write_text(content)
    elif kind == "terminal_echo":
        # The agent supplies data only. This program and output path are host-owned.
        target = root / "terminal.txt"
        if target.is_symlink():
            raise ValueError("Symlink target refused")
        result = subprocess.run(
            [sys.executable, "-I", "-S", "-B", "-c", "import sys; print(sys.argv[1], end='')", content],
            env={}, capture_output=True, timeout=2, check=True,
        )
        if result.stderr or len(result.stdout) > 8192:
            raise ValueError("Unexpected terminal output")
        target.write_bytes(result.stdout)
    elif kind == "mcp_echo":
        target = root / "mcp.txt"
        if target.is_symlink():
            raise ValueError("Symlink target refused")
        target.write_text(content)
    else:
        raise ValueError("Unsupported operation")
    return {"content": [{"type": "text", "text": "Host effect finished; independent verification is still required."}]}


def sandbox_operation(arguments):
    """Use only the host-prepared invocation; agent arguments can't select its profile."""
    context_bytes = pathlib.Path(os.environ['GAAP_SANDBOX_CONTEXT']).read_bytes()
    if len(context_bytes) > 65536:
        raise ValueError('Oversized sandbox context')
    context = json.loads(context_bytes)
    if context['arguments'] != arguments:
        raise ValueError('Sandbox arguments differ from the admitted invocation')
    limits = context['request']['limits']
    response_seconds = (limits['wall_clock_ms'] + limits['cleanup_ms']) / 1000 + 1
    cancellation_seconds = limits['cleanup_ms'] / 1000 + 1
    child = subprocess.Popen([context['executable'], context['config']], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env={}, start_new_session=True)
    try:
        output, errors = child.communicate(json.dumps(context['request']).encode(), timeout=response_seconds)
    except subprocess.TimeoutExpired:
        os.killpg(child.pid, signal.SIGTERM)
        try:
            output, errors = child.communicate(timeout=cancellation_seconds)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate()
            raise ValueError('Sandbox response is uncertain; inspect its owned intent before recovery')
    if child.returncode or errors or len(output) > 1024 * 1024:
        raise ValueError('Sandbox transport failed; retained ownership must be inspected')
    observation = json.loads(output)
    descriptor = os.open(context['observation'], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, 'wb') as retained:
        retained.write(output)
        retained.flush()
        os.fsync(retained.fileno())
    succeeded = observation['status'] == 'exited' and observation['exit_code'] == 0 and observation['cleanup'] == 'removed' and observation['publication_error'] is None
    return {'content': [{'type': 'text', 'text': 'Host OCI observation retained; independent verification is still required.'}], 'isError': not succeeded}


def proposal(arguments):
    """Wait for the owning host without performing any proposed effect."""
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
        channel.settimeout(40)
        channel.connect(sys.argv[2])
        channel.sendall((json.dumps({"token": sys.argv[3], "arguments": arguments}) + "\n").encode())
        with channel.makefile("rb") as reader:
            line = reader.readline(16385)
        if len(line) > 16384 or not line.endswith(b"\n"):
            raise ValueError("Invalid host response")
        return json.loads(line)


def main():
    """Serve pinned MCP 2025-11-25; reject other lifecycle variants."""
    role = sys.argv[1]
    if role not in ("ingress", "effect"):
        raise ValueError("Unknown server role")
    initialized = False
    while True:
        line = sys.stdin.buffer.readline(32769)
        if not line:
            return
        if len(line) > 32768 or not line.endswith(b"\n"):
            return
        request = json.loads(line)
        if "id" not in request:
            continue
        method = request.get("method")
        try:
            if method == "initialize" and request["params"]["protocolVersion"] == "2025-11-25":
                initialized = True
                result = {"protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                          "serverInfo": {"name": "gaap-" + ("proposals" if role == "ingress" else "effect"), "version": "1"}}
            elif method == "tools/list" and initialized:
                result = {"tools": [TOOL]}
            elif method == "tools/call" and initialized and request["params"]["name"] == TOOL["name"]:
                arguments = request["params"]["arguments"]
                result = proposal(arguments) if role == "ingress" else operation(arguments)
            elif method == "ping":
                result = {}
            else:
                raise LookupError("Unsupported method")
            response = {"jsonrpc": "2.0", "id": request["id"], "result": result}
        except LookupError:
            response = {"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "Unsupported method"}}
        except Exception:
            response = {"jsonrpc": "2.0", "id": request["id"], "result": {"content": [{"type": "text", "text": "Reference operation failed"}], "isError": True}}
        print(json.dumps(response), flush=True)


if __name__ == "__main__":
    main()

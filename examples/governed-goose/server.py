"""Proposal ingress and separate, host-approved effect backend for the reference."""
import json
import os
import signal
import selectors
import time
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


def capture_sandbox(command, payload, response_seconds, cancellation_seconds):
    """Bound both pipes and retain transport facts independently of JSON validity."""
    child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, env={}, start_new_session=True)
    streams = selectors.DefaultSelector()
    pending = memoryview(payload)
    captured = {'stdout': bytearray(), 'stderr': bytearray()}
    limits = {'stdout': 1024 * 1024, 'stderr': 8192}
    truncated = {'stdout': False, 'stderr': False}
    termination = None
    deadline = time.monotonic() + response_seconds
    stage = 0
    for name in ('stdin', 'stdout', 'stderr'):
        pipe = getattr(child, name)
        os.set_blocking(pipe.fileno(), False)
        streams.register(pipe, selectors.EVENT_WRITE if name == 'stdin' else selectors.EVENT_READ, name)
    try:
        while streams.get_map() or child.poll() is None:
            now = time.monotonic()
            if now >= deadline:
                if stage == 0:
                    termination = termination or 'response_timeout'
                    signum, stage, delay = signal.SIGTERM, 1, cancellation_seconds
                elif stage == 1:
                    signum, stage, delay = signal.SIGKILL, 2, 1
                else:
                    termination = 'unconfirmed_transport_cleanup'
                    break
                try:
                    os.killpg(child.pid, signum)
                except ProcessLookupError:
                    pass
                deadline = now + delay
            for key, _events in streams.select(timeout=0.01):
                name, pipe = key.data, key.fileobj
                if name == 'stdin':
                    try:
                        written = os.write(pipe.fileno(), pending[:65536])
                        pending = pending[written:]
                    except BrokenPipeError:
                        pending = pending[:0]
                    if not pending:
                        streams.unregister(pipe)
                        pipe.close()
                    continue
                chunk = os.read(pipe.fileno(), 65536)
                if not chunk:
                    streams.unregister(pipe)
                    pipe.close()
                    continue
                room = limits[name] - len(captured[name])
                captured[name].extend(chunk[:room])
                if len(chunk) > room:
                    truncated[name] = True
                    termination = 'output_overflow'
                    if stage == 0:
                        deadline = time.monotonic()
        child.poll()
        return {'schema_version': 'gaap.sandbox-transport/0.1',
                'exit_code': child.returncode, 'termination': termination,
                'stdout': list(captured['stdout']), 'stderr': list(captured['stderr']),
                'stdout_truncated': truncated['stdout'], 'stderr_truncated': truncated['stderr']}
    finally:
        streams.close()
        for name in ('stdin', 'stdout', 'stderr'):
            pipe = getattr(child, name)
            if not pipe.closed:
                pipe.close()
        if child.poll() is None:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            try:
                child.wait(timeout=1)
            except subprocess.TimeoutExpired:
                pass


def retain_private(path, content):
    """Publish one host-selected record without replacing retained evidence."""
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, 'wb') as retained:
        retained.write(content)
        retained.flush()
        os.fsync(retained.fileno())
    parent = os.open(str(pathlib.Path(path).parent), os.O_RDONLY)
    try:
        os.fsync(parent)
    finally:
        os.close(parent)


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
    transport = capture_sandbox([context['executable'], context['config']],
                               json.dumps(context['request']).encode(),
                               response_seconds, cancellation_seconds)
    transport['request_digest'] = context['request_digest']
    retain_private(context['transport'], json.dumps(transport).encode())
    if transport['exit_code'] != 0 or transport['stderr'] or transport['termination']:
        raise ValueError('Sandbox transport failed; inspect its retained transport and ownership evidence')
    output = bytes(transport['stdout'])
    observation = json.loads(output)
    retain_private(context['observation'], output)
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

"""Run real pinned Goose against an unbilled deterministic provider fixture."""
import argparse
import hashlib
import http.server
import json
import shutil
import os
import pathlib
import platform
import signal
import subprocess
import sys
import tempfile
import threading

HERE = pathlib.Path(__file__).resolve().parent
PINS = {
    ("Darwin", "arm64"): "4b8258c24adad9e07957d1dddfc7d965b97883dce2eff64af7e3d3c005655d81",
    ("Linux", "x86_64"): "2419b124b0b1eb4e02b19cbc2373a91e8927010c8ed2902680c7a2864a49785f",
}
GOOD = """def shipping_quote(quantity, unit_price):
    if quantity <= 0:
        raise ValueError('quantity must be positive')
    subtotal = quantity * unit_price
    if quantity >= 5:
        subtotal = subtotal * 0.9
    if subtotal < 100:
        subtotal = subtotal + 7
    return round(subtotal, 2)
"""
BAD = "def shipping_quote(quantity, unit_price):\n    return 0\n"


def file_hash(path):
    """Hash launch bytes without loading the Goose binary into memory."""
    digest = hashlib.sha256()
    with pathlib.Path(path).open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def artifact(path):
    """Bind an absolute launch artifact to its current bytes."""
    path = pathlib.Path(path).resolve()
    return {"path": str(path), "digest": file_hash(path)}


def provider(case, transcript, workspace):
    """Serve protocol fixtures only; there's no inference or external billing."""
    class Provider(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            """Keep raw HTTP logs outside the reproducible evidence."""

        def do_GET(self):
            """Advertise the single deterministic fixture model."""
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps({"object": "list", "data": [{"id": "gaap-fixture", "object": "model", "owned_by": "fixture"}]}).encode())

        def do_POST(self):
            """Emit a bounded sequence of three tool proposals or a false claim."""
            length = int(self.headers["Content-Length"])
            if length > 1024 * 1024:
                self.send_error(413)
                return
            body = json.loads(self.rfile.read(length))
            tools = body.get("tools", [])
            transcript.append({"tools": tools, "messages": body.get("messages", []), "stream": body.get("stream")})
            names = [tool["function"]["name"] for tool in tools]
            if names != ["gaap-proposals__governed_operation"] or not body.get("stream"):
                self.send_error(422)
                return
            completed = sum(message.get("role") == "tool" for message in body.get("messages", []))
            operations = [
                {"operation": "file_write", "content": BAD.replace("return 0", "return 1") if case == "bad_patch" else GOOD},
                {"operation": "terminal_echo", "content": "terminal witnessed"},
                {"operation": "mcp_echo", "content": "mcp witnessed"},
            ]
            if completed < len(operations) and case != "report_only":
                call = {"index": 0, "id": "proposal-" + str(completed + 1), "type": "function",
                        "function": {"name": names[0], "arguments": json.dumps(operations[completed])}}
                steps = [({"role": "assistant", "tool_calls": [call]}, None), ({}, "tool_calls")]
            else:
                if case == "unreported":
                    (workspace / "unreported.txt").write_text("Actual out-of-band mutation without a Protected Effect Request.\n")
                elif case == "unreported_directory":
                    (workspace / "unreported-directory").mkdir()
                elif case == "unreported_mode":
                    target = workspace / "shipping.py"
                    target.chmod(target.stat().st_mode ^ 0o100)
                steps = [({"role": "assistant", "content": "The task is complete."}, None), ({}, "stop")]
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            for delta, finish in steps:
                chunk = {"id": "fixture", "object": "chat.completion.chunk", "created": 1, "model": "gaap-fixture",
                         "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
                self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
    return http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)


def run_host(command, timeout=55):
    """Bound one host invocation and stop its controlled process group on every exit."""
    with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          start_new_session=True, env={"PATH": os.defpath}) as process:
        try:
            output, errors = process.communicate(timeout=timeout)
            if process.returncode:
                raise RuntimeError("Host failed: " + errors.decode()[:2000])
            return output
        finally:
            # The controlled host and its children share this new process group.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=3)


def run_case(case, goose, binary, output, attestor=None, bind_controller=False, sandbox=None):
    """Own one disposable repository, provider, conversation and terminal receipt."""
    host_binary_digest = file_hash(binary)
    root = pathlib.Path(tempfile.mkdtemp(prefix="gg-", dir="/tmp")).resolve()
    workspace = root / "repository"
    workspace.mkdir()
    state = root / "agent-state"
    state.mkdir()
    (state / "config").mkdir()
    config_file = state / "config" / "config.yaml"
    config_file.write_text("extensions:\n  developer:\n    enabled: false\n    type: builtin\n    name: developer\n")
    (workspace / "shipping.py").write_text(BAD)
    if sandbox is not None:
        (workspace / "terminal.txt").write_text('')
        (workspace / "mcp.txt").write_text('')
        sandbox = dict(sandbox)
        sandbox['state_directory'] = str(root / 'sandbox-state')
        pathlib.Path(sandbox['state_directory']).mkdir(mode=0o700)
        if os.getuid() == 0:
            uid, gid = map(int, sandbox['user'].split(':'))
            for name in ('shipping.py', 'terminal.txt', 'mcp.txt'):
                os.chown(workspace / name, uid, gid)
    templates = root / "git-templates"
    templates.mkdir()
    subprocess.run(["git", "init", "--quiet", str(workspace)], check=True,
                   env={"PATH": os.defpath,"HOME":str(state),"GIT_CONFIG_NOSYSTEM":"1",
                        "GIT_CONFIG_GLOBAL":os.devnull,"GIT_TEMPLATE_DIR":str(templates)})
    transcript = []
    server = provider(case, transcript, workspace)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    python = pathlib.Path(sys.executable).resolve()
    token = os.urandom(32).hex()
    socket_path = root / "ingress.sock"
    evidence = output / (case + ".json")
    environment = {"HOME": str(state), "PATH": os.defpath, "GOOSE_PATH_ROOT": str(state),
                   "GOOSE_PROVIDER": "openai", "GOOSE_MODEL": "gaap-fixture", "GOOSE_MODE": "auto",
                   "OPENAI_BASE_URL": "http://127.0.0.1:" + str(server.server_port) + "/v1"}
    agent = {"executable": str(goose), "arguments": ["acp"], "environment": environment,
             "working_directory": str(workspace), "expected_name": "goose", "expected_version": "1.54.0",
             "artifacts": [artifact(goose), artifact(config_file)]}
    ingress = {"executable": str(python), "arguments": [str(HERE / "server.py"), "ingress", str(socket_path), token],
               "environment": {}, "working_directory": str(workspace), "expected_name": "gaap-proposals", "expected_version": "1",
               "artifacts": [artifact(python), artifact(HERE / "server.py")]}
    manifest = root / "host.json"
    configuration = {"agent": agent, "proposal_server": ingress, "python": str(python),
                     "server": str(HERE / "server.py"), "socket": str(socket_path), "token": token,
                     "output": str(evidence), "case": case}
    if sandbox is not None:
        configuration['sandbox'] = sandbox
    manifest.write_text(json.dumps(configuration))
    try:
        print("Running " + case + " with real Goose 1.54.0", file=sys.stderr, flush=True)
        template = json.loads(run_host([str(binary), "--describe", str(manifest)], timeout=25 if sandbox is not None else 15))
        if transcript or evidence.exists() or socket_path.exists() or (workspace / "shipping.py").read_text() != BAD:
            raise AssertionError("Discovery started a conversation or changed the reference workspace")
        if bind_controller:
            template["request_id"] = "controller-request-" + case
            template["run_id"] = "controller-run-" + case
            configuration = json.loads(manifest.read_text())
            configuration["agent_run_request"] = template
            manifest.write_text(json.dumps(configuration))
        run_host([str(binary), str(manifest)], timeout=150 if sandbox is not None else 55)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)
    if file_hash(binary) != host_binary_digest:
        raise RuntimeError("The host binary changed during this case; rerun from a stable build")
    record = json.loads(evidence.read_text())
    if record["request"] != template:
        raise AssertionError("The executed request differs from its selected discovery template")
    if bind_controller:
        retained = evidence.read_bytes()
        refreshed = json.loads(run_host([str(binary), "--describe", str(manifest)], timeout=25 if sandbox is not None else 15))
        if refreshed["subject"] != record["after"] or refreshed["subject"] == template["subject"]:
            raise AssertionError("Discovery didn't refresh the changed subject independently of the old bound request")
        try:
            run_host([str(binary), str(manifest)], timeout=15)
        except RuntimeError as error:
            if "Controller request differs" not in str(error):
                raise AssertionError("A stale bound request failed for the wrong reason") from error
        else:
            raise AssertionError("A stale bound request started another run")
        if evidence.read_bytes() != retained:
            raise AssertionError("Stale-request rejection overwrote the retained native evidence")
    receipt = record["receipt"]["body"]
    expected = {"allow": "completed", "ask": "blocked", "block": "blocked", "bad_patch": "blocked", "report_only": "failed", "unreported": "blocked", "unreported_directory": "blocked", "unreported_mode": "blocked", "cancel_before_dispatch":"interrupted"}[case]
    if receipt["terminal_status"] != expected:
        raise AssertionError(case + ": unexpected receipt status " + receipt["terminal_status"])
    effects = record["protected_effect_results"]
    if len(record["protected_effect_requests"]) != len(effects) or len(record["raw_proposals"]) != len(effects):
        raise AssertionError("The exported evidence lost a proposed request or raw input")
    attempted = sum(effect["body"]["execution_status"] == "executed" for effect in effects)
    if case in ("ask", "block"):
        response = record["ingress_responses"][0]
        expected_reason = "permission.policy_requires_approval" if case == "ask" else "permission.denied"
        expected_effect = "awaiting_authority" if case == "ask" else "denied"
        if receipt["terminal_reason"] != expected_reason or effects[0]["body"]["execution_status"] != expected_effect:
            raise AssertionError("The intended permission gate didn't stop this proposal")
        if response["response"]["structuredContent"]["status"] != "not_dispatched" or not response["written"]:
            raise AssertionError("The host didn't return its no-dispatch response to the ingress")
    if case == "cancel_before_dispatch":
        response = record["ingress_responses"][0]
        if response["response"]["structuredContent"]["status"] != "not_dispatched" or not isinstance(response.get("written"), bool):
            raise AssertionError("The early refusal lost its pending proposal response")
    if case in ("ask", "block", "report_only", "cancel_before_dispatch"):
        if attempted or (workspace / "shipping.py").read_text() != BAD or ((workspace / "terminal.txt").read_text() != '' or (workspace / "mcp.txt").read_text() != '' if sandbox is not None else (workspace / "terminal.txt").exists() or (workspace / "mcp.txt").exists()):
            raise AssertionError("Refused or fabricated work caused a protected effect")
    elif attempted != 3:
        raise AssertionError("The full conversation didn't execute all three admitted operations")
    if case in ("bad_patch", "unreported", "unreported_directory", "unreported_mode") and record["verification"]["passed"] is not False:
        raise AssertionError("Independent verification didn't reject the bad patch")
    if case == "allow" and record["verification"]["passed"] is not True:
        raise AssertionError("Independent verification didn't pass")
    if sandbox is not None:
        observations = [value for value in record['evidence_blobs'].values()
                        if isinstance(value, list) and value and value[0] == ord('{')]
        decoded = []
        for value in observations:
            try:
                entry = json.loads(bytes(value))
            except (ValueError, UnicodeDecodeError):
                continue
            if isinstance(entry, dict) and entry.get('schema_version') == 'gaap.sandbox-observation/0.1':
                decoded.append(entry)
        expected_count = 0 if case in ('ask', 'block', 'report_only', 'cancel_before_dispatch') else 4
        if len(decoded) != expected_count or any(entry['cleanup'] != 'removed' or (entry.get('runtime') or {}).get('runtime') != 'runc' for entry in decoded):
            raise AssertionError('OCI runtime or cleanup mismatch: ' + json.dumps([{'status': entry.get('status'), 'reason': entry.get('reason'), 'cleanup': entry.get('cleanup'), 'runtime': entry.get('runtime')} for entry in decoded]))
        if any(result['body']['sandbox_profile']['name'] != 'governed-goose-local-oci' for result in effects if result['body']['executor'] is not None):
            raise AssertionError('A protected effect reported the wrong selected profile')
    record["fixture"] = {"goose_version": "1.54.0", "binary_digest": file_hash(goose),
                         "host_binary_digest": host_binary_digest,
                         "host_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=str(HERE), text=True).strip(),
                         "controller_request_bound": bind_controller,
                         "sandbox_profile": 'local-oci/0.1' if sandbox is not None else 'local-stdio-unisolated',
                         "host_tree_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=str(HERE), text=True).strip()),
                         "provider": "local deterministic fixture; no inference or external billing",
                         "repository": str(workspace), "case": case, "provider_requests": transcript,
                         "fault_injection": case + ": provider fixture changed the working tree after three governed effects" if case.startswith("unreported") else None}
    evidence.write_text(json.dumps(record, indent=2))
    if attestor is not None:
        signed = output / (case + "-signed")
        subprocess.run([str(attestor), "fixture-sign", str(evidence), str(signed), host_binary_digest], check=True)
        verified = subprocess.check_output([str(attestor), "verify", str(signed / "envelope.json"),
                                           str(signed / "fixture-trust.json"), str(signed / "fixture-policies.json"),
                                           str(signed / "blobs")], text=True)
        check = json.loads(verified)
        if check["terminal_status"] != receipt["terminal_status"] or check["controller_authority"] != "none":
            raise AssertionError("Signing changed the run's meaning")
        (signed / "verification.json").write_text(verified)
    # Leave the disposable repository for evidence inspection; it contains no credentials.
    return {"case": case, "status": receipt["terminal_status"], "executed_effects": attempted, "evidence": str(evidence)}


def main():
    """Verify the binary pin, build the host and reproduce all reference outcomes."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--goose", required=True, type=pathlib.Path, help="Checksum-pinned Goose 1.54.0 executable")
    parser.add_argument("--output", required=True, type=pathlib.Path, help="New directory for the nine-case evidence")
    parser.add_argument('--oci', action='store_true', help='Run the same matrix through the pinned local OCI profile')
    args = parser.parse_args()
    goose = args.goose.resolve(strict=True)
    expected = PINS.get((platform.system(), platform.machine()))
    if expected is None or file_hash(goose) != "sha256:" + expected:
        raise ValueError("Use the checksum-pinned Goose 1.54.0 binary for this platform")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    subprocess.run(["cargo", "build", "--locked", "--manifest-path", str(HERE / "Cargo.toml")], check=True)
    binary = HERE / "target" / "debug" / "gaap-governed-goose"
    attestation_manifest = HERE.parent.parent / "adapters" / "attestation" / "Cargo.toml"
    subprocess.run(["cargo", "build", "--locked", "--manifest-path", str(attestation_manifest)], check=True)
    attestor = attestation_manifest.parent / "target" / "debug" / "gaap-attestation"
    sandbox = None
    if args.oci:
        sandbox_manifest = HERE.parent.parent / 'adapters' / 'sandbox' / 'Cargo.toml'
        subprocess.run(['cargo', 'build', '--locked', '--manifest-path', str(sandbox_manifest)], check=True)
        docker_name = shutil.which('docker')
        if docker_name is None:
            raise ValueError('Install a Docker-compatible local CLI before selecting --oci')
        docker = pathlib.Path(docker_name).resolve(strict=True)
        version = json.loads(subprocess.check_output([str(docker), 'version', '--format', '{{json .Server}}']))
        endpoint = subprocess.check_output([str(docker), 'context', 'inspect', '--format', '{{.Endpoints.docker.Host}}'], text=True).strip()
        image_digest = 'sha256:2d9aefe2fef018a7eb2c13064c89c71929800fd2e5dccdbf52ea5da5bb8d929a'
        image = 'python@' + image_digest
        subprocess.run([str(docker), 'image', 'inspect', image], stdout=subprocess.DEVNULL, check=True)
        sandbox = {'executable': str((sandbox_manifest.parent / 'target' / 'debug' / 'gaap-sandbox').resolve(strict=True)),
                   'docker_executable': str(docker), 'docker_environment': {'DOCKER_HOST': endpoint},
                   'engine_version': version['Version'], 'runtime_version': next(component['Version'] for component in version['Components'] if component['Name'] == 'runc'),
                   'image': {'reference': image, 'digest': image_digest},
                   'user': str(os.getuid() or 1000) + ':' + str(os.getgid() or 1000)}
    results = [run_case(case, goose, binary, output, attestor, sandbox=sandbox) for case in ("allow", "ask", "block", "bad_patch", "report_only", "unreported", "unreported_directory", "unreported_mode", "cancel_before_dispatch")]
    bound_output = output / "controller-bound"
    bound_output.mkdir()
    bound = run_case("allow", goose, binary, bound_output, attestor, bind_controller=True, sandbox=sandbox)
    results.append({**bound, "case": "controller_bound"})
    (output / "summary.json").write_text(json.dumps(results, indent=2))
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()

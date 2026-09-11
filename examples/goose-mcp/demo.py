#!/usr/bin/env python3
"""Local operator launcher for the GAAP conference example."""

import getpass
import os
from pathlib import Path
import sys


def configure():
    """Accept an API key through a hidden terminal prompt, never through argv."""
    destination = credential_path("gaap-demo", "openai-key")
    directory = destination.parent
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    if directory.is_symlink():
        raise SystemExit("Refusing a symlinked credential directory.")
    if destination.exists() or destination.is_symlink():
        raise SystemExit(f"A credential already exists at {destination}; leaving it unchanged.")
    if not sys.stdin.isatty():
        raise SystemExit("Run configure in your own terminal; the key is entered with echo disabled.")
    key = getpass.getpass("OpenAI API key (hidden): ").strip()
    if not key or any(char.isspace() for char in key):
        raise SystemExit("No key saved: expected one nonempty key without whitespace.")
    fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as handle:
        handle.write(key)
        handle.flush()
        os.fsync(handle.fileno())
    print(f"Saved locally with owner-only access: {destination}")

import argparse
import contextlib
import datetime
import difflib
import json
import shlex
import signal
import stat
import subprocess
import tempfile
import time
import urllib.request

from install_goose import verify_binary

HERE = Path(__file__).resolve().parent
BINARY = HERE / "target" / "debug" / "gaap-goose-demo"
GOOSE = Path.home() / ".local" / "share" / "gaap-demo" / "tools" / "goose-1.50.0" / "goose"
DEFAULT_MODELS = {"chatgpt_codex": "gpt-5.5", "openai": "gpt-5.4-mini"}


def credential_path(*parts):
    """Reject credential redirection at every component from the user's home."""
    components = (".config", *parts)
    path = Path.home().joinpath(*components)
    if any(component.is_symlink() for component in [path, *list(path.parents)[:len(components)]]):
        raise SystemExit("Credential path component must not be a symlink.")
    return path


def codex_token(timeout):
    """Read Goose's existing sign-in without refreshing or changing its credentials."""
    path = credential_path("goose", "chatgpt_codex", "tokens.json")
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except FileNotFoundError:
        raise SystemExit("Sign in to ChatGPT Codex in Goose first, then retry.") from None
    with os.fdopen(fd) as handle:
        metadata = os.fstat(handle.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077 or metadata.st_uid != os.getuid():
            raise SystemExit("Goose sign-in must be an owner-only regular file owned by you.")
        raw = handle.read(65537)
    try:
        if len(raw) > 65536:
            raise ValueError("oversized token file")
        data = json.loads(raw)
        expires = datetime.datetime.fromisoformat(data["expires_at"].replace("Z", "+00:00"))
        if expires <= datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(seconds=timeout + 120):
            raise SystemExit("Goose sign-in expires too soon. Refresh the connection in Goose, then retry.")
        if not all(isinstance(data.get(key), str) and data[key] for key in ["access_token", "account_id"]):
            raise ValueError("missing access token or account")
    except (ValueError, KeyError, TypeError, AttributeError):
        raise SystemExit("Goose sign-in data is invalid; reconnect in Goose.") from None
    # No refresh authority or identity token is copied. This bounded invocation
    # finishes before expiry and cannot rotate the desktop application's token.
    return {"access_token": data["access_token"], "account_id": data["account_id"],
            "expires_at": data["expires_at"], "refresh_token": "", "id_token": None}


@contextlib.contextmanager
def provider_environment(goose_root, provider, model, timeout):
    """Provide isolated per-call configuration and remove its credential copy on exit."""
    with tempfile.TemporaryDirectory(prefix="run-", dir=goose_root) as directory:
        env = {
            "HOME": str(Path.home()), "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            "LANG": "en_US.UTF-8", "GOOSE_PATH_ROOT": directory,
            "GOOSE_PROVIDER": provider, "GOOSE_MODEL": model, "GOOSE_MODE": "auto",
            "GOOSE_DISABLE_KEYRING": "1", "NO_COLOR": "1",
        }
        if provider == "chatgpt_codex":
            token = codex_token(timeout)
            destination = Path(directory) / "config" / "chatgpt_codex"
            destination.mkdir(mode=0o700, parents=True)
            fd = os.open(destination / "tokens.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "w") as handle:
                json.dump(token, handle)
            secrets = [token["access_token"], token["account_id"]]
        else:
            key = key_from_file()
            env["OPENAI_API_KEY"] = key
            secrets = [key]
        yield env, secrets


def binary(*args, capture=False):
    """Run the local Rust command and optionally decode its JSON output."""
    if not BINARY.is_file():
        raise SystemExit("Build first: python3 demo.py build")
    result = subprocess.run([str(BINARY), *map(str, args)], text=True, capture_output=capture, check=True)
    return json.loads(result.stdout) if capture else None


def key_from_file():
    """Read the owner-only local credential without exposing it in arguments or output."""
    path = credential_path("gaap-demo", "openai-key")
    if not path.exists():
        raise SystemExit("Local OpenAI key is missing. Run: python3 demo.py configure")
    if path.parent.is_symlink():
        raise SystemExit("Credential directory must not be a symlink.")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd) as handle:
        metadata = os.fstat(handle.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077 or metadata.st_uid != os.getuid():
            raise SystemExit("Credential must be an owner-only regular file owned by you.")
        key = handle.read(4097).strip()
    if not key or len(key) > 4096 or any(c.isspace() for c in key):
        raise SystemExit("Credential file does not contain a valid single key.")
    return key


def session(root):
    """Resolve a session root and require valid operator state before using it."""
    root = Path(root).resolve(strict=True)
    private_directory(root)
    binary("inspect", root / "operator", capture=True)
    return root


def private_directory(path, create=False):
    """Require owner-only directories before storing provider state or transcripts."""
    if create:
        path.mkdir(mode=0o700, exist_ok=True)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
    try:
        metadata = os.fstat(fd)
        if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
            raise SystemExit(f"Expected an owner-only directory owned by you: {path}")
    finally:
        os.close(fd)


def terminal_text(value):
    """Render controls and backslashes visibly while preserving original stored bytes."""
    parts = []
    for char in value:
        if char == "\\":
            parts.append("\\\\")
        elif char == "\n" or char.isprintable():
            parts.append(char)
        elif ord(char) < 256:
            parts.append(f"\\x{ord(char):02x}")
        elif ord(char) <= 65535:
            parts.append(f"\\u{ord(char):04x}")
        else:
            parts.append(f"\\U{ord(char):08x}")
    return "".join(parts)


def summary(root):
    """Display recorded decisions and the Rust inspector's checked file observation."""
    root = session(root)
    state = binary("inspect", root / "operator", capture=True)
    print("Goose view - MCP boundary decisions (not an engine terminal receipt)")
    for request_id, entry in state["proposals"].items():
        print(f"{request_id}\n  approved={entry['approved']} state={entry['execution_status']}")
    for record in state["records"]:
        data = record["data"]
        decision = data.get("decision", {})
        if decision:
            print(f"  {record['sequence']:02d} {record['kind']}: {decision['outcome']} / {decision['code']} / {data['execution_status']}")
        elif record["kind"] == "execution_observation":
            print(f"  {record['sequence']:02d} observed: {data['execution_status']}")
    print(f"Observed executions: {state['execution_count']}; uncertain requests: {len(state['uncertain_requests'])}")
    observation = state["current_observation"]
    if "error" in observation:
        print(f"Current file observation unavailable: {observation['error']}")
    else:
        print(terminal_text(observation["content"]))
    return state


def engine_view(scenario, full_json=False):
    """Render actual engine events compactly or return the complete JSON view."""
    result = binary("engine", scenario, capture=True)
    if full_json:
        print(json.dumps(result, indent=2))
        return
    print(result["view"])
    print(f"Scenario: {scenario}; usage and evidence are deterministic fixtures")
    for event in result["receipt"]["body"]["events"]:
        kind = event["event_type"]
        if "decision" in event:
            decision = event["decision"]
            detail = f"{event['gate']}: {decision['outcome']} / {decision['code']}"
        elif kind == "status_transition":
            detail = f"{event['from']} -> {event['to']}"
        elif kind == "verification":
            detail = f"{event['verdict']} by {event['verifier_id']}"
        elif kind == "usage":
            detail = json.dumps(event["usage"], sort_keys=True)
        elif kind == "mutation":
            detail = "in-memory artifact changed; before/after subject digests recorded"
        else:
            detail = kind.replace("_", " ")
        print(f"  {event['sequence']:02d}  {detail}")
    receipt = result["receipt"]
    print(f"Terminal: {receipt['body']['terminal_status']} / {receipt['body']['terminal_reason']}")
    print(f"Artifact version: {result['artifact_version']}")
    print(f"Receipt verified: {result['terminal_receipt_verified']} - {receipt['receipt_digest']}")
    print("Use --json for the complete receipt and Protected Effect Results.")


def stop_process(process):
    """Stop the entire process group, escalate if needed, and collect remaining output."""
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        output, _ = process.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        output, _ = process.communicate()
    return output


def goose(root, prompt, timeout=90, provider="chatgpt_codex", model=None):
    """Run the pinned goose with one MCP extension and preserve a redacted transcript."""
    root = session(root)
    verify_binary(GOOSE)
    model = model or DEFAULT_MODELS[provider]
    goose_root = root / "goose"
    private_directory(goose_root, create=True)
    with provider_environment(goose_root, provider, model, timeout) as (env, secrets):
        return run_goose(root, prompt, timeout, provider, model, env, secrets)


def run_goose(root, prompt, timeout, provider, model, env, secrets, *, mcp_entry=None, max_turns=6):
    """Execute the verified runtime and capture the actual provider-backed result."""
    entry = mcp_entry or [sys.executable, str(HERE / "demo.py"), "mcp-server", str(root)]
    extension = "gaap:" + shlex.join(entry)
    command = [str(GOOSE), "run", "--no-profile", "--no-session", "--provider", provider, "--model", model,
               "--with-extension", extension, "--max-turns", str(max_turns), "--max-tool-repetitions", "2",
               "--output-format", "text", "--text", prompt]
    logs = root / "transcripts"
    private_directory(logs, create=True)
    stamp = str(time.time_ns())
    transcript = logs / f"goose-{stamp}.txt"
    print(f"Goose view - real goose 1.50.0 / {provider} {model} / only the gaap MCP extension", flush=True)
    started = time.monotonic()
    process = subprocess.Popen(command, cwd=root / "workspace", env=env, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, start_new_session=True)
    failure = None
    try:
        output, _ = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        failure = "timeout"
        output = stop_process(process)
    except BaseException:
        failure = "interrupted"
        output = stop_process(process)
    if failure:
        output = (output or "") + f"\nOPERATOR {failure.upper()}: inspect state before continuing.\n"
    for secret in secrets:
        output = output.replace(secret, "[REDACTED]")
        prompt = prompt.replace(secret, "[REDACTED]")
    elapsed = time.monotonic() - started
    header = f"REAL GOOSE RUN (terminal controls and backslashes escaped for display)\nProvider: {provider}\nModel: {model}\nExtensions: only gaap; --no-profile\nPrompt: {prompt}\n\n"
    with transcript.open("x") as handle:
        handle.write(terminal_text(header + output + f"\nProcess exit: {process.returncode}; elapsed_seconds: {elapsed:.2f}\n"))
    print(terminal_text(output), end="" if output.endswith("\n") else "\n")
    print(f"Transcript: {transcript} ({elapsed:.1f}s)")
    if failure is not None or process.returncode != 0:
        raise SystemExit("Goose did not finish successfully. Inspect the transcript and operator state; no success was inferred.")
    return transcript


def propose_prompt(version):
    """Build the bounded version-change instruction used for real rehearsals."""
    return (f"Read release.json using the gaap read_file tool. Update only its version to {version}. "
            "Call propose_write with the complete new JSON content. Return the exact request_id and decision, "
            "then stop for operator approval. Do not call apply_change and do not claim the file changed.")


def find_proposal(root, version):
    """Find one real recorded proposal for the expected version or stop the rehearsal."""
    state = binary("inspect", root / "operator", capture=True)
    matches = [(key, entry) for key, entry in state["proposals"].items()
               if json.loads(entry["proposal"]["content"]).get("version") == version]
    if len(matches) != 1:
        raise SystemExit(f"Expected exactly one proposal for {version}; inspect the real run.")
    return matches[0][0]


def desktop(root, provider="chatgpt_codex", model=None):
    """Open a fresh, three-tool recipe in the installed Goose desktop application."""
    verify_binary(GOOSE)
    root = Path(root).absolute()
    binary("init", root)
    root = session(root)
    recipe = {
        "version": "1.0.0",
        "title": "Governed autonomy: Goose + GAAP",
        "description": "Propose a release version change, inspect GAAP's decision, then apply only after exact operator approval.",
        "instructions": (
            "Demonstrate governed file operations using the three GAAP tools. "
            "read_file observes release.json. propose_write records complete proposed contents without changing the file. "
            "When approval is required, report the exact request_id and decision, then stop. "
            "Only the human operator can approve in a separate terminal. "
            "Call apply_change only when explicitly asked, using the exact recorded request_id. "
            "After applying, report the decision and read the actual file. "
            "If a request is blocked, report that decision and stop; do not propose an alternative unless asked. "
            "These are MCP boundary decision records, not full AgentRunEngine terminal receipts."
        ),
        "activities": [
            "message: Goose requests the operation. GAAP checks authority. The operator approves the exact proposal in a separate terminal.",
            propose_prompt("1.1.0"),
            "Read release.json and report its actual version.",
        ],
        "extensions": [{
            "type": "stdio", "name": "gaap", "cmd": sys.executable,
            "args": [str(HERE / "demo.py"), "mcp-server", str(root)],
            "env_keys": [], "timeout": 30,
            "description": "GAAP governed release.json operations; approval is operator-only.",
            "available_tools": ["read_file", "propose_write", "apply_change"],
        }],
        "settings": {"goose_provider": provider, "goose_model": model or DEFAULT_MODELS[provider], "max_turns": 6},
    }
    path = root / "desktop-recipe.json"
    with path.open("x") as handle:
        json.dump(recipe, handle, indent=2)
    print(f"Desktop recipe: {path}", flush=True)
    print(f"Operator commands use this root: {root}", flush=True)
    print("Review the recipe in Goose, then choose Trust and Execute. Only the gaap extension is listed.", flush=True)
    subprocess.run([str(GOOSE), "recipe", "open", str(path)], check=True)


def rehearse(root, provider="chatgpt_codex", model=None):
    """Run and assert the approved real goose sequence in a fresh disposable session."""
    model = model or DEFAULT_MODELS[provider]
    def run(prompt):
        """Keep every phase on the same selected provider and model."""
        return goose(root, prompt, provider=provider, model=model)
    root = Path(root).absolute()
    started = time.monotonic()
    binary("init", root)
    root = session(root)
    original = (root / "workspace" / "release.json").read_text()
    run(propose_prompt("1.1.0"))
    request_id = find_proposal(root, "1.1.0")
    if (root / "workspace" / "release.json").read_text() != original:
        raise SystemExit("Rehearsal failed: fixture changed before operator approval.")
    print("CHECK: proposal requested approval; fixture is unchanged.")
    binary("approve", root / "operator", request_id, "--yes")
    run(f"Use apply_change for request_id {request_id}. Then read release.json. Report the actual decision and observed version.")
    state = summary(root)
    if state["execution_count"] != 1 or json.loads((root / "workspace" / "release.json").read_text())["version"] != "1.1.0":
        raise SystemExit("Rehearsal failed: approved change was not observed exactly once.")
    print(terminal_text("".join(difflib.unified_diff(original.splitlines(True), (root / "workspace" / "release.json").read_text().splitlines(True), fromfile="before/release.json", tofile="after/release.json"))))
    run(propose_prompt("1.2.0"))
    stale_id = find_proposal(root, "1.2.0")
    binary("approve", root / "operator", stale_id, "--yes")
    # Explicit operator action in the disposable fixture, to make approved authority stale.
    path = root / "workspace" / "release.json"
    fd = os.open(path, os.O_WRONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "w") as handle:
        if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
            raise SystemExit("Fixture must be a regular file.")
        handle.truncate(0)
        handle.write('{"name":"conference-demo","version":"9.0.0"}\n')
        handle.flush()
        os.fsync(handle.fileno())
    print("OPERATOR changed the underlying file to 9.0.0. Predict the next decision.", flush=True)
    run(f"Call apply_change for request_id {stale_id} exactly once. Do not propose another change. Read release.json and report the decision and actual file version.")
    state = summary(root)
    decisions = [record["data"] for record in state["records"] if record["kind"] == "apply_decision" and record["data"]["request_id"] == stale_id]
    if not decisions or decisions[-1]["decision"]["code"] != "demo.stale_subject" or state["execution_count"] != 1:
        raise SystemExit("Rehearsal failed: expected a recorded stale-subject block and exactly one effect.")
    if json.loads(path.read_text())["version"] != "9.0.0":
        raise SystemExit("Rehearsal failed: operator edit was not preserved.")
    elapsed = time.monotonic() - started
    result = {"type":"real-goose-rehearsal", "passed":elapsed < 420, "elapsed_seconds":round(elapsed,2),
              "provider":provider, "model":model, "goose_version":"1.50.0", "approved_request":request_id,
              "stale_request":stale_id, "execution_count":state["execution_count"], "state_dir":str(root / "operator")}
    destination = root / "transcripts" / "rehearsal.json"
    with destination.open("x") as handle:
        json.dump(result, handle, indent=2)
    print(json.dumps(result, indent=2))
    if not result["passed"]:
        raise SystemExit("Correct behavior, but the rehearsal exceeded seven minutes.")


def main():
    """Parse and execute the local operator command."""
    parser = argparse.ArgumentParser(description="Governed autonomy: Engine view and real goose MCP view")
    parser.add_argument("--provider", choices=list(DEFAULT_MODELS), default="chatgpt_codex")
    parser.add_argument("--model", help="explicit model override; defaults to the selected provider model")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("configure", help="save the API key through a hidden local terminal prompt")
    commands.add_parser("build", help="build the isolated locked Rust example")
    commands.add_parser("doctor", help="check pinned goose, binary, and selected provider authentication")
    desktop_command = commands.add_parser("desktop", help="create a fresh fixture and open its restricted recipe in Goose Desktop")
    desktop_command.add_argument("root", help="new disposable session directory")
    engine = commands.add_parser("engine", help="show deterministic AgentRunEngine events and receipt")
    engine.add_argument("--json", action="store_true", help="print the complete receipt and effect results as JSON")
    engine.add_argument("scenario", nargs="?", default="completed", choices=["completed","approval-required","stale-verification"])
    for name in ["init", "inspect", "status", "rehearse", "mcp-server"]:
        cmd = commands.add_parser(name)
        cmd.add_argument("root", help="disposable session directory")
    approve = commands.add_parser("approve", help="display exact diff and record operator approval")
    approve.add_argument("root")
    approve.add_argument("request_id")
    approve.add_argument("--yes", action="store_true", help="explicitly approve without a prompt")
    run = commands.add_parser("goose", help="make real model-driven requests through the isolated MCP extension")
    run.add_argument("root")
    run.add_argument("--text", required=True)
    args = parser.parse_args()
    model = args.model or DEFAULT_MODELS[args.provider]
    if args.command == "configure":
        configure()
    elif args.command == "build":
        subprocess.run(["cargo","build","--locked","--manifest-path",str(HERE / "Cargo.toml")],check=True)
    elif args.command == "doctor":
        verify_binary(GOOSE)
        print("Verified goose 1.50.0 executable checksum", flush=True)
        binary("--help")
        if args.provider == "chatgpt_codex":
            codex_token(90)
            print(f"Goose ChatGPT Codex sign-in available; selected model: {model}. Run rehearse to verify live access.")
            return
        key = key_from_file()
        request = urllib.request.Request(f"https://api.openai.com/v1/models/{model}", headers={"Authorization":f"Bearer {key}"})
        try:
            with urllib.request.urlopen(request,timeout=30) as response:
                metadata = json.load(response)
            print(f"OpenAI model accessible: {metadata['id']}")
        except Exception as error:
            raise SystemExit(f"Model access check failed ({type(error).__name__}); check your local API account.") from None
    elif args.command == "engine":
        engine_view(args.scenario, args.json)
    elif args.command == "desktop":
        desktop(args.root, provider=args.provider, model=model)
    elif args.command == "init":
        binary("init", Path(args.root).absolute())
    elif args.command == "inspect":
        binary("inspect", session(args.root) / "operator")
    elif args.command == "status":
        summary(args.root)
    elif args.command == "approve":
        root = session(args.root)
        state = binary("inspect",root / "operator",capture=True)
        entry = state["proposals"].get(args.request_id)
        if entry is None:
            raise SystemExit("Unknown request_id. Use status or inspect to see stored proposals.")
        proposal = entry["proposal"]
        print("Exact diff (controls and backslashes escaped for display):", flush=True)
        print(terminal_text("".join(difflib.unified_diff((proposal["before"] or "").splitlines(True), proposal["content"].splitlines(True), fromfile="before/release.json", tofile="proposed/release.json"))), flush=True)
        binary("approve",root / "operator",args.request_id,*(["--yes"] if args.yes else []))
    elif args.command == "goose":
        goose(args.root, args.text, provider=args.provider, model=model)
    elif args.command == "rehearse":
        rehearse(args.root, provider=args.provider, model=model)
    elif args.command == "mcp-server":
        # Server inherits neither the provider key nor the operator's ambient environment.
        root = Path(args.root).resolve(strict=True)
        os.execve(str(BINARY),[str(BINARY),"serve","--state-dir",str(root / "operator")],{"PATH":"/usr/bin:/bin","LANG":"en_US.UTF-8"})


if __name__ == "__main__":
    try:
        main()
    except (OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"Demo command failed: {error}") from None

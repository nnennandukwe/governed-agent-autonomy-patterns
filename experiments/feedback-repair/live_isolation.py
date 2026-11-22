"""Constrain public request construction and final scoring to separate macOS processes."""

import base64
import ctypes
import errno
import hashlib
import inspect
import json
import os
from pathlib import Path
import platform
import socket
import subprocess
import sys
import tempfile
import time

import corpus
import recorder

HERE = Path(__file__).resolve().parent
PUBLIC_FIELDS = ("id", "target", "prompt", "initial", "checks")
DEFAULT_SECONDS = 20
PROJECTION_PROOF_VERSION = "gaap.feedback-isolation/1"


def _apply_policy(policy, sealed_source):
    """Enter the sandbox and confirm this role can't open the scorer source before handoff."""
    library = ctypes.CDLL("/usr/lib/libsandbox.dylib", use_errno=True)
    library.sandbox_init.argtypes = [ctypes.c_char_p, ctypes.c_uint64, ctypes.POINTER(ctypes.c_char_p)]
    error = ctypes.c_char_p()
    if library.sandbox_init(policy.encode(), 0, ctypes.byref(error)) != 0:
        raise RuntimeError("The operating system rejected the study sandbox.")
    try:
        descriptor = os.open(sealed_source, os.O_RDONLY)
    except PermissionError:
        return
    os.close(descriptor)
    raise RuntimeError("The worker can open the scorer source.")


def _policy(paths):
    """Permit reads of named files and their path metadata with other operations denied."""
    rules = ["(version 1)", "(deny default)"]
    for path in paths:
        # Callers qualify the directory. Keep each permitted child name literal,
        # even if a substituted link currently leads elsewhere.
        if not Path(path).is_absolute() or ".." in Path(path).parts:
            raise ValueError("Sandbox rules need absolute, qualified paths.")
        literal = json.dumps(str(path), ensure_ascii=False)
        rules.append(f"(allow file-read* file-test-existence (literal {literal}))")
        rules.append(f"(allow file-read-metadata file-test-existence (path-ancestors {literal}))")
    return "\n".join(rules)


def _probe(payload):
    """Test prohibited reads, modifications, outbound connections and inherited handles."""
    denials = {}
    for action, path, flags in (("private_read", payload["private"], os.O_RDONLY),
                                ("other_cell_read", payload["other"], os.O_RDONLY),
                                ("credential_read", payload["credential"], os.O_RDONLY),
                                ("scorer_read", payload["scorer"], os.O_RDONLY),
                                ("protected_write", payload["protected"], os.O_WRONLY)):
        try:
            descriptor = os.open(path, flags)
        except OSError as error:
            denials[action] = error.errno
        else:
            os.close(descriptor)
            denials[action] = 0
    try:
        with socket.socket() as outbound:
            outbound.settimeout(1)
            outbound.connect(("127.0.0.1", payload["port"]))
    except OSError as error:
        denials["network"] = error.errno
    else:
        denials["network"] = 0
    try:
        metadata = os.fstat(payload["inheritable_fd"])
    except OSError:
        leaked_descriptor = False
    else:
        leaked_descriptor = [metadata.st_dev, metadata.st_ino] == payload["inheritable_identity"]
    return {"denials": denials, "inherited_canary_fd": leaked_descriptor,
            "environment_names": sorted(os.environ), "pid": os.getpid()}


def _custody_action(role, payload):
    """Read one committed task and expose public fields or score a terminal selection."""
    task = corpus.read_member(Path(payload["directory"]), payload["entry"])
    if role == "prepare":
        return {name: task[name] for name in ("id", "target", "prompt", "initial", "checks")}
    chosen = payload["selected"]
    selected = base64.b64decode(chosen, validate=True) if chosen is not None else None
    return corpus.evaluate(task, payload["before"], Path(payload["workspace"]), selected)


def _request_action(payload):
    """Build a tool-free request from the three public files and one allowed prior reply."""
    initial = common_request(payload["study"], {"target": "settings.json"},
                             Path(payload["workspace"]), payload["initial_check"])
    return request_for(payload["study"], initial, payload["arm"], payload["prior"])


def _runtime_files():
    """Inventory the clean child's loaded Python modules, bytecode and native libraries."""
    paths = {Path(sys.executable).resolve()}
    for module in tuple(sys.modules.values()):
        for attribute in ("__file__", "__cached__"):
            location = getattr(module, attribute, None)
            if location and Path(location).is_file():
                paths.add(Path(location).resolve())
    system = ctypes.CDLL(None)
    system._dyld_get_image_name.argtypes = [ctypes.c_uint32]
    system._dyld_get_image_name.restype = ctypes.c_char_p
    for ordinal in range(system._dyld_image_count()):
        location = os.fsdecode(system._dyld_get_image_name(ordinal))
        # macOS serves these libraries from its shared cache, bound by the OS build.
        if not location.startswith(("/System/", "/usr/lib/")):
            paths.add(Path(location).resolve(strict=True))
    return {str(path): corpus.digest(path.read_bytes()) for path in sorted(paths)}


def _worker_source(role):
    """Assemble trusted startup code before allowing a child to consume its sole input."""
    imports = "import base64, ctypes, json, os, socket, sys\nfrom pathlib import Path\n"
    if role in ("prepare", "score", "identity"):
        # Embed the frozen scorer bytes in this process image. It opens no fixtures at startup.
        frozen = (HERE / "corpus.py").read_bytes()
        imports += ("import types\ncorpus = types.ModuleType('corpus')\n"
                    + f"corpus.__file__ = {str(HERE / 'corpus.py')!r}\n"
                    + f"exec({frozen!r}, corpus.__dict__)\n")
        if role == "identity":
            return imports + inspect.getsource(_runtime_files) + "\nprint(json.dumps(_runtime_files()))\n"
        action = inspect.getsource(_custody_action)
        expression = f"_custody_action({role!r}, payload)"
    elif role == "request":
        action = (inspect.getsource(recorder.common_request) + inspect.getsource(recorder.request_for)
                  + inspect.getsource(_request_action))
        expression = "_request_action(payload)"
    elif role == "probe":
        action = inspect.getsource(_probe)
        expression = "_probe(payload)"
    else:
        raise ValueError("Unknown isolation role.")
    return (imports + inspect.getsource(_apply_policy) + action
            + "\nos.environ.clear()\n_apply_policy(sys.argv[1], sys.argv[2])\npayload = json.load(sys.stdin)\n"
            + f"print(json.dumps({expression}, ensure_ascii=True, allow_nan=False))\n")


def _run(role, payload, read_paths, deadline=None):
    """Launch a sterile child with pipe-only handoff and a bounded monotonic deadline."""
    if sys.platform != "darwin":
        raise RuntimeError("Live isolation requires a qualified macOS runner.")
    cutoff = time.monotonic() + DEFAULT_SECONDS
    if deadline is not None:
        cutoff = min(cutoff, deadline)
    command = [str(Path(sys.executable).resolve()), "-I", "-S", "-B", "-c",
               _worker_source(role), _policy(read_paths), str(HERE / "corpus.py")]
    request = json.dumps(payload, allow_nan=False).encode()
    timeout = cutoff - time.monotonic()
    if timeout <= 0:
        raise TimeoutError("Isolation deadline expired.")
    try:
        child = subprocess.run(command, input=request,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd="/",
                               env={}, close_fds=True, timeout=timeout)
    except subprocess.TimeoutExpired as error:
        raise TimeoutError("Isolation process timed out.") from error
    if time.monotonic() > cutoff:
        raise TimeoutError("Isolation deadline expired.")
    if child.returncode:
        # Child diagnostics can contain private input or file paths.
        raise RuntimeError(f"The isolated {role} process failed with status {child.returncode}.")
    return json.loads(child.stdout)


def profile():
    """Identify the interpreter, sources, role programs and operating-system sandbox."""
    if sys.platform != "darwin":
        raise RuntimeError("Live isolation requires a qualified macOS runner.")
    executable = Path(sys.executable).resolve()
    build = subprocess.check_output(["/usr/bin/sw_vers", "-buildVersion"], env={}, timeout=5).decode().strip()
    dependencies = _run("identity", {}, [])
    return {"schema": PROJECTION_PROOF_VERSION, "platform": sys.platform, "machine": platform.machine(),
            "os_build": build, "kernel": platform.release(), "python": sys.version,
            "executable": str(executable), "executable_sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
            "runtime_file_count": len(dependencies),
            "runtime_files_sha256": corpus.digest(corpus.encode(dependencies)),
            "source_sha256": {name: hashlib.sha256((HERE / name).read_bytes()).hexdigest()
                              for name in ("live_isolation.py", "corpus.py", "recorder.py", "study.json")},
            "worker_sha256": {role: hashlib.sha256(_worker_source(role).encode()).hexdigest()
                              for role in ("prepare", "request", "score", "probe")},
            "policy": "deny default; named read-only files and path metadata; sterile pipes"}


def _member(private_dir, member):
    """Resolve an opaque member only through the frozen index."""
    entry = next((item for item in corpus._index() if item["id"] == member), None)
    if entry is None:
        raise ValueError("Unknown frozen member.")
    directory = HERE / "fixtures" if entry["split"] == "development" else Path(private_dir)
    if directory.is_symlink():
        raise ValueError("Member custody requires a direct directory.")
    directory = directory.resolve(strict=True)
    return entry, directory


def prepare(private_dir, member, workspace, deadline=None):
    """Reset one selected workspace from the custodian's public projection and return its digests."""
    entry, directory = _member(private_dir, member)
    projected = _run("prepare", {"directory": str(directory), "entry": entry},
                     [directory, directory / (member + ".json")], deadline)
    if set(projected) != set(PUBLIC_FIELDS) or projected["id"] != member or projected["target"] != corpus.TARGET:
        raise ValueError("Unexpected public member projection.")
    return projected, corpus.reset(projected, workspace)


def candidate_request(projected_task, arm, history, initial_check, deadline=None):
    """Build each request in a new read-only workspace without scorer bytes or cross-cell inputs."""
    if (set(projected_task) != set(PUBLIC_FIELDS) or projected_task["target"] != corpus.TARGET
            or arm not in recorder.ARMS or type(initial_check) is not bool or type(history) is not list):
        raise ValueError("Invalid public request inputs.")
    prior = None
    if history:
        if len(history) != 1 or arm not in ("revision", "lesson"):
            raise ValueError("This strategy can't receive that history.")
        prior = history[0]
        if (type(prior) is not dict or set(prior) != {"reply_hex", "public_pass"}
                or prior["public_pass"] is not False or type(prior["reply_hex"]) is not str
                or len(prior["reply_hex"]) > 2 * corpus.MAX_BUNDLE_BYTES):
            raise ValueError("Invalid revision parent.")
        raw = bytes.fromhex(prior["reply_hex"])
        if not raw or raw.hex() != prior["reply_hex"]:
            raise ValueError("Invalid revision bytes.")
    study = json.loads((HERE / "study.json").read_bytes())
    public_study = {"base_prompt": study["base_prompt"],
                    "arms": [{"id": item["id"], "prompt": item["prompt"]}
                             for item in study["arms"] if item["id"] == arm]}
    with tempfile.TemporaryDirectory() as name:
        workspace = Path(name).resolve()
        corpus.reset(projected_task, workspace)
        paths = [workspace] + [workspace / filename for filename in sorted(corpus.WORKSPACE_NAMES)]
        return _run("request", {"arm": arm, "prior": prior, "initial_check": initial_check,
                                "study": public_study, "workspace": str(workspace)}, paths, deadline)


def score(private_dir, member, before, workspace, selected, deadline=None):
    """Score one closed selection in a separate read-only process with the committed original."""
    entry, directory = _member(private_dir, member)
    root = Path(workspace)
    if root.is_symlink():
        raise ValueError("Selected workspace must be a direct directory.")
    root = root.resolve(strict=True)
    # Never resolve a substituted target into a new read grant.
    paths = [directory, directory / (member + ".json"), root]
    paths.extend(root / name for name in sorted(corpus.WORKSPACE_NAMES))
    encoded = None if selected is None else base64.b64encode(selected).decode("ascii")
    return _run("score", {"directory": str(directory), "entry": entry, "before": before,
                          "workspace": str(root), "selected": encoded}, paths, deadline)


def qualify():
    """Observe denial and public request parity using disposable canaries and the actual runner."""
    identity = profile()
    task, gold, _ = corpus.controls()[0]
    projected = {name: task[name] for name in PUBLIC_FIELDS}
    with tempfile.TemporaryDirectory() as name:
        root = Path(name).resolve()
        workspace = root / "public"
        prepared, before = prepare(None, task["id"], workspace)
        if prepared != projected:
            raise RuntimeError("Custody changed the public task projection.")
        paths = [workspace] + [workspace / filename for filename in sorted(corpus.WORKSPACE_NAMES)]
        canaries = {key: root / key for key in ("private", "other", "credential")}
        for path in canaries.values():
            path.write_bytes(os.urandom(32))
        with canaries["credential"].open("rb") as inherited, socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen(1)
            os.set_inheritable(inherited.fileno(), True)
            metadata = os.fstat(inherited.fileno())
            denial = _run("probe", {**{key: str(path) for key, path in canaries.items()},
                                    "scorer": str(HERE / "corpus.py"), "protected": str(workspace / "TASK.txt"),
                                    "port": listener.getsockname()[1], "inheritable_fd": inherited.fileno(),
                                    "inheritable_identity": [metadata.st_dev, metadata.st_ino]}, paths)
        if (any(result not in (errno.EPERM, errno.EACCES) for result in denial["denials"].values())
                or denial["inherited_canary_fd"] or denial["environment_names"]
                or corpus.snapshot(workspace) != before):
            raise RuntimeError("The isolation controls failed.")
        initial_pass = corpus.public_checks(projected, corpus.encode(projected["initial"]))
        request = candidate_request(projected, "raw", [], initial_pass)
        expected = recorder.common_request(json.loads((HERE / "study.json").read_bytes()),
                                           projected, workspace, initial_pass)
        if request != expected:
            raise RuntimeError("The isolated worker changed the frozen request.")
        (workspace / corpus.TARGET).write_bytes(gold)
        result = score(None, task["id"], before, workspace, gold)
        if not result["correct"] or not result["effect_valid"]:
            raise RuntimeError("The independent scorer lost a valid public repair.")
        return {"schema": PROJECTION_PROOF_VERSION, "profile": identity,
                "denials": denial["denials"], "inherited_canary_fd": denial["inherited_canary_fd"],
                "environment_names": denial["environment_names"],
                "public_request_sha256": corpus.digest(recorder.encoded(request)),
                "public_score_sha256": corpus.digest(corpus.encode(result))}

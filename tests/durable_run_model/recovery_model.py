"""Design fixture for ADR 0009. Each snapshot replacement models one durable publication.

The scripted target has a separate commit history. The host can read an attributed
witness only after that target fences the invocation. No production engine runs.
"""

from contextlib import contextmanager
from dataclasses import asdict, dataclass
import fcntl
from hashlib import sha256
import json
from threading import Lock


class Refusal(Exception):
    """A fixture guard refused the operation."""


class Cut(Exception):
    """Control never returns to the interrupted invocation."""


def encode(value):
    """Encode this fixture's string-valued bindings; frozen contract hashes are external."""
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def digest(text):
    """Hash the exact UTF-8 bytes of one scripted artifact."""
    return sha256(text.encode("utf-8")).hexdigest()


@dataclass(frozen=True)
class Submission:
    """Symbolic identities and proposal bytes for one explicit bounded attempt."""

    key: str
    run: str
    attempt: str
    effect: str
    proposal: str
    authority: str
    subject: str
    replacement: str
    policy: str = "policy-1"
    capability: str = "capability-1"
    verifier: str = "independent-verifier-1"

    def binding(self, caller):
        """Bind the authenticated caller and every proposal field to one byte string."""
        return encode({"caller": caller, **asdict(self)})


class Target:
    """Script a single cooperating target and an independently attributable witness."""

    def __init__(self):
        """Start the independent target at a known value."""
        self.lock = Lock()
        self.content = "original"
        self.commits = []
        self.intervals = {}
        self.witnesses = {}
        self.scratch = set()

    def open_interval(self, binding):
        """Register this invocation at the independent target before delivery."""
        with self.lock:
            if binding in self.intervals:
                raise Refusal("Target invocation already registered")
            self.intervals[binding] = "not_executed"

    def invoke(self, binding, request, fail):
        """Perform one committed replacement or a known pre-commit executor failure."""
        with self.lock:
            if binding in self.witnesses or self.intervals[binding] != "not_executed":
                raise Refusal("Target invocation fenced or consumed")
            self.scratch.add(binding)
            if fail or digest(self.content) != request.subject:
                self.intervals[binding] = "failed"
                return "failed"
            self.content = request.replacement
            self.commits.append(binding)
            self.intervals[binding] = "executed"
            return "executed"

    def fence(self, binding):
        """Close a registered interval and certify its outcome against later writes."""
        with self.lock:
            outcome = self.intervals[binding]
            self.witnesses[binding] = {"binding": binding, "outcome": outcome}
            return self.witnesses[binding].copy()


def initialize(root):
    """Create a known empty fixture journal; opening an existing host never initializes it."""
    with (root / "journal.json").open("x", encoding="utf-8") as stream:
        json.dump({"version": 1, "records": []}, stream)
    (root / "lock").touch(exist_ok=False)


def inspect(root):
    """Read one published generation without acquiring execution authority or doing cleanup."""
    snapshot = json.loads((root / "journal.json").read_text(encoding="utf-8"))
    if snapshot["version"] != 1:
        raise Refusal("Unsupported journal version; preserve the store")
    return snapshot


def publish(root, snapshot):
    """Replace a fixture snapshot as a unit; the cut model assumes completed writes survive."""
    staged = root / "journal.next"
    staged.write_text(encode(snapshot), encoding="utf-8")
    staged.replace(root / "journal.json")


def uninterrupted(point):
    """Let the scripted invocation pass the named cut point."""


def unresolved(record):
    """Check whether this workspace still needs an independently admitted ruling."""
    return record["effect"] == "unknown" and record["ruling"] is None


class Host:
    """Model serialized journal writes, fresh authority checks and read-before-retry."""

    def __init__(self, root, target):
        """Open an existing journal with its independent target and trusted host settings."""
        self.root = root
        self.target = target
        self.grants = {}
        self.policy = "policy-1"
        self.capability = "capability-1"
        self.verifier = "independent-verifier-1"

    @contextmanager
    def writer(self):
        """Serialize all cooperating writers using one existing lock for the workspace."""
        with (self.root / "lock").open("rb") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            yield inspect(self.root)

    def grant(self, caller, request):
        """Script an independently issued current grant for precisely this binding."""
        self.grants[request.authority] = (request.binding(caller), "current")

    def gate(self, binding, request):
        """Evaluate trusted authority, current policy, capability, subject and verifier."""
        grant_binding, state = self.grants.get(request.authority, (None, "missing"))
        if state != "current":
            return f"authority_{state}"
        if grant_binding != binding:
            return "authority_mismatch"
        if (request.policy, request.capability, request.verifier) != (
            self.policy, self.capability, self.verifier
        ):
            return "configuration_drift"
        if request.subject != digest(self.target.content):
            return "stale_subject"
        return None

    def submit(self, caller, request, *, at=uninterrupted, script="normal"):
        """Read the identity under lock before deciding whether a first invocation can run."""
        binding = request.binding(caller)
        at("before_lock")
        with self.writer() as snapshot:
            for previous in snapshot["records"]:
                if (previous["caller"], previous["key"]) == (caller, request.key):
                    if previous["binding"] != binding:
                        raise Refusal("Identity conflict; preserve the original binding")
                    return previous
            for previous in snapshot["records"]:
                if unresolved(previous):
                    raise Refusal("Workspace unresolved; admit independent evidence")
                original = json.loads(previous["binding"])
                if any(original[field] == getattr(request, field)
                       for field in ("run", "attempt", "effect", "proposal", "authority")):
                    raise Refusal("Use fresh identities and fresh authority")
            at("before_intent")
            record = dict(caller=caller, key=request.key, binding=binding, stage="intent",
                          effect="not_executed", verification=None, receipt=None,
                          charged_dispatches=0, ruling=None)
            snapshot["records"].append(record)
            publish(self.root, snapshot)
            at("before_dispatch")
            refusal = self.gate(binding, request)
            if refusal:
                terminal, reason = "blocked", refusal
            elif script == "cancel_before_dispatch":
                terminal, reason = "interrupted", "cancelled"
            else:
                terminal, reason = self.dispatch(snapshot, record, request, script, at)
            at("during_receipt")
            record.update(stage="published", receipt={"status": terminal, "reason": reason,
                                                      "effect": record["effect"]})
            publish(self.root, snapshot)
            try:
                at("during_cleanup")
                self.target.scratch.discard(binding)
            except OSError:
                return {**record, "warning": "Owned staging cleanup failed"}
            at("before_response")
            return record

    def dispatch(self, snapshot, record, request, script, at):
        """Cross the dispatch fence once and order independent observation before verification."""
        binding = record["binding"]
        record.update(stage="dispatch", effect="unknown", charged_dispatches=1)
        publish(self.root, snapshot)
        self.target.open_interval(binding)
        at("before_effect")
        outcome = self.target.invoke(binding, request, script == "executor_failure")
        at("after_effect")
        if script == "unknown_report":
            return "failed", "unknown_outcome"
        record.update(stage="observed", effect=outcome)
        publish(self.root, snapshot)
        if outcome == "failed":
            return "failed", "executor_failure"
        if script == "cancel_after_effect":
            return "interrupted", "cancelled"
        at("during_verification")
        passed = script != "verification_failure"
        record.update(stage="verified", verification="passed" if passed else "failed")
        publish(self.root, snapshot)
        return ("completed", "verified") if passed else ("blocked", "verification_failed")

    def admit(self, actor, caller, request, witness):
        """Append one host ruling for an unresolved invocation after its target fences it."""
        if actor != "host-reconciler":
            raise Refusal("Only the host reconciler may admit evidence")
        binding = request.binding(caller)
        with self.writer() as snapshot:
            record = next((entry for entry in snapshot["records"] if entry["binding"] == binding),
                          None)
            if record is None or record["effect"] != "unknown":
                raise Refusal("Only an unresolved dispatch can receive evidence")
            expected = self.target.witnesses.get(binding)
            if expected is None or witness != expected:
                raise Refusal("Supply a final independent witness for this invocation")
            if record["ruling"] is None:
                record["ruling"] = {"actor": actor, **witness}
                publish(self.root, snapshot)
            return record

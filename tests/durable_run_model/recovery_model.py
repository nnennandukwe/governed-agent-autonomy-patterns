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
    request_id: str
    authority: str
    subject: str
    replacement: str
    policy: str = "policy-1"
    capability: str = "capability-1"
    verifier: str = "independent-verifier-1"

    def binding(self, caller, enrollment):
        """Bind the authenticated caller and every proposal field to one byte string."""
        proposal_bytes = encode({"subject": self.subject, "replacement": self.replacement})
        return encode({**enrollment, "caller": caller, "proposal_digest": digest(proposal_bytes),
                       **asdict(self)})


class Target:
    """Script a single cooperating target and an independently attributable witness."""

    def __init__(self, root, store_id, target_id):
        """Enroll one target at a fixed fixture store with host-issued symbolic identities."""
        self.enrolled_root = root.resolve()
        self.enrollment = {"store_id": store_id, "target_id": target_id}
        self.lock = Lock()
        self.content = "original"
        self.commits = []
        self.intervals = {}
        self.witnesses = {}
        self.scratch = set()

    def open_interval(self, binding):
        """Register this invocation at the independent target before delivery."""
        with self.lock:
            identity = json.loads(binding)
            if any(identity[key] != value for key, value in self.enrollment.items()):
                raise Refusal("Foreign target enrollment")
            if binding in self.intervals:
                raise Refusal("Target invocation already registered")
            self.intervals[binding] = ("not_executed", None)

    def invoke(self, binding, request, fail):
        """Perform one committed replacement or a known pre-commit executor failure."""
        with self.lock:
            if binding != request.binding(json.loads(binding)["caller"], self.enrollment):
                raise Refusal("Invocation bytes conflict with registered binding")
            if binding in self.witnesses or self.intervals.get(binding) != ("not_executed", None):
                raise Refusal("Target invocation fenced or consumed")
            self.scratch.add(binding)
            if fail or digest(self.content) != request.subject:
                self.intervals[binding] = ("failed", None)
                return "failed"
            self.content = request.replacement
            self.commits.append(binding)
            self.intervals[binding] = ("executed", digest(self.content))
            return "executed"

    def fence(self, binding):
        """Close a registered interval and certify its outcome against later writes."""
        with self.lock:
            outcome, output_subject = self.intervals[binding]
            self.witnesses[binding] = {"binding": binding, "outcome": outcome,
                                       "output_subject": output_subject, **self.enrollment}
            return self.witnesses[binding].copy()


def require_record(condition, field):
    """Reject damaged fixture state before any lookup or dispatch decision."""
    if not condition:
        raise Refusal(f"Invalid journal {field}; preserve the store")


def validate_ruling(ruling, record, enrollment, output):
    """Check the saved reconciliation decision against its invocation and expected output."""
    require_record(isinstance(ruling, dict) and set(ruling) == {
        "actor", "binding", "store_id", "target_id", "outcome", "output_subject"
    }, "ruling fields")
    require_record(record["effect"] == "unknown" and ruling["actor"] == "host-reconciler"
                   and ruling["binding"] == record["binding"]
                   and all(ruling[key] == value for key, value in enrollment.items()), "ruling binding")
    outcome = ruling["outcome"]
    require_record(outcome in ("executed", "failed", "not_executed") and
                   ruling["output_subject"] == (output if outcome == "executed" else None), "ruling outcome")


def validate_receipt(record):
    """Require receipt status, reason and effect to describe a reachable terminal decision."""
    receipt = record["receipt"]
    if record["stage"] != "published":
        require_record(receipt is None, "unpublished receipt")
        return
    effect, verdict = record["effect"], record["verification"]
    require_record(isinstance(receipt, dict) and set(receipt) == {"effect", "status", "reason"}
                   and receipt["effect"] == effect, "published receipt")
    status, reason = receipt["status"], receipt["reason"]
    require_record(isinstance(reason, str) and isinstance(status, str), "receipt fields")
    allowed = {
        "completed": effect == "executed" and verdict == "passed" and reason == "verified",
        "failed": record["reserved_dispatches"] == 1 and verdict is None and reason == ("unknown_outcome" if effect == "unknown"
                                                 else "executor_failure"),
        "interrupted": effect in ("executed", "not_executed") and verdict is None and reason == "cancelled",
        "blocked": ((effect == "not_executed" and (reason.startswith("authority_") or
                     reason in ("configuration_drift", "stale_subject"))) or
                    (verdict, reason) in (("failed", "verification_failed"), ("passed", "stale_verification")))
    }
    require_record(allowed.get(status, False), "receipt outcome")


def validate_record(record, enrollment):
    """Check one miniature record and return its identity claims for cross-record checks."""
    fields = {"caller", "key", "binding", "stage", "effect", "observed_subject", "verified_subject",
              "verification", "receipt", "reserved_dispatches", "ruling"}
    require_record(isinstance(record, dict) and set(record) - {"dispute"} == fields, "record fields")
    identity = json.loads(record["binding"])
    request = Submission(**{key: identity[key] for key in Submission.__dataclass_fields__})
    require_record(all(isinstance(value, str) for value in identity.values()) and
                   all(value for key, value in identity.items() if key != "replacement") and
                   record["key"] == request.key and record["caller"] == identity["caller"] and
                   record["binding"] == request.binding(record["caller"], enrollment), "canonical binding")

    stage, effect, verdict = record["stage"], record["effect"], record["verification"]
    allowed_effects = {
        "intent": ("not_executed",), "dispatch": ("unknown",),
        "observed": ("not_executed", "failed", "executed"), "verified": ("executed",),
        "published": ("not_executed", "failed", "executed", "unknown")
    }
    require_record(effect in allowed_effects.get(stage, ()), "stage and effect")
    reserved = record["reserved_dispatches"]
    require_record(type(reserved) is int and reserved in (0, 1) and
                   (stage == "published" or reserved == int(stage != "intent")) and
                   (reserved == 1 or effect == "not_executed"), "dispatch reservation")
    output = digest(request.replacement)
    require_record(record["observed_subject"] == (output if effect == "executed" else None),
                   "observed output")
    if verdict is None:
        require_record(stage != "verified" and record["verified_subject"] is None, "absent verdict")
    else:
        subject = record["verified_subject"]
        require_record(stage in ("verified", "published") and effect == "executed" and
                       verdict in ("passed", "failed") and isinstance(subject, str) and
                       len(subject) == 64 and all(char in "0123456789abcdef" for char in subject)
                       and (verdict != "passed" or subject == output), "verification")

    validate_receipt(record)

    ruling, dispute = record["ruling"], record.get("dispute")
    if ruling is not None:
        validate_ruling(ruling, record, enrollment, output)
    if "dispute" in record:
        require_record(ruling is not None, "dispute without ruling")
        validate_ruling(dispute, record, enrollment, output)
        require_record(ruling["outcome"] != dispute["outcome"], "conflicting evidence")
    claims = {(field, identity[field]) for field in ("request_id", "run", "attempt", "effect", "authority")}
    claims.add(("submission", record["caller"], record["key"]))
    return claims


def journal_bytes(snapshot):
    """Bind the fixture generation to its bytes; this unkeyed checksum detects accidental damage."""
    return encode({**snapshot, "checksum": digest(encode(snapshot))})


def initialize(root, target):
    """Create a known empty fixture journal; opening an existing host never initializes it."""
    with (root / "journal.json").open("x", encoding="utf-8") as stream:
        stream.write(journal_bytes({"version": 1, "enrollment": target.enrollment, "records": []}))
    (root / "lock").touch(exist_ok=False)


def inspect(root):
    """Read and validate a complete published generation without acquiring execution authority."""
    try:
        raw = (root / "journal.json").read_text(encoding="utf-8")
        snapshot = json.loads(raw)
        require_record(isinstance(snapshot, dict), "format")
        if type(snapshot.get("version")) is not int or snapshot["version"] != 1:
            raise Refusal("Unsupported journal version; preserve the store")
        require_record(set(snapshot) == {"version", "enrollment", "records", "checksum"}, "format")
        del snapshot["checksum"]
        require_record(raw == journal_bytes(snapshot), "checksum or encoding")
        enrollment = snapshot["enrollment"]
        require_record(isinstance(enrollment, dict) and set(enrollment) == {"store_id", "target_id"}
                       and all(isinstance(value, str) and value for value in enrollment.values()), "enrollment")
        require_record(isinstance(snapshot["records"], list), "record list")
        seen = set()
        for record in snapshot["records"]:
            claims = validate_record(record, enrollment)
            require_record(seen.isdisjoint(claims), "duplicate identity")
            seen.update(claims)
        return snapshot
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise Refusal("Unreadable or malformed journal; preserve the store") from error


def publish(root, snapshot, *, receipt_cut=None):
    """Replace a fixture snapshot as a unit; the cut model assumes completed writes survive."""
    staged = root / "journal.next"
    staged.write_text(journal_bytes(snapshot), encoding="utf-8")
    if receipt_cut is not None:
        receipt_cut("during_receipt")
    staged.replace(root / "journal.json")


def uninterrupted(point):
    """Let the scripted invocation pass the named cut point."""


def unresolved(record):
    """Check whether this workspace still needs an independently admitted ruling."""
    return record["effect"] == "unknown" and (record["ruling"] is None or "dispute" in record)


class Host:
    """Model serialized journal writes, fresh authority checks and read-before-retry."""

    def __init__(self, root, target):
        """Open an existing journal with its independent target and trusted host settings."""
        self.root = root
        self.target = target
        self.check_enrollment(inspect(root))
        self.grants = {}
        self.policy = "policy-1"
        self.capability = "capability-1"
        self.verifier = "independent-verifier-1"

    def check_enrollment(self, snapshot):
        """Refuse a store or target from another enrollment, including after reopening."""
        if (self.root.resolve() != self.target.enrolled_root
                or snapshot["enrollment"] != self.target.enrollment):
            raise Refusal("Store and target enrollment mismatch")

    @contextmanager
    def writer(self):
        """Serialize all cooperating writers using one existing lock for the workspace."""
        with (self.root / "lock").open("rb") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            snapshot = inspect(self.root)
            self.check_enrollment(snapshot)
            yield snapshot

    def grant(self, caller, request):
        """Script an independently issued current grant for precisely this binding."""
        self.grants[request.authority] = (request.binding(caller, self.target.enrollment), "current")

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
        binding = request.binding(caller, self.target.enrollment)
        at("before_lock")
        with self.writer() as snapshot:
            for previous in snapshot["records"]:
                if (previous["caller"], previous["key"]) == (caller, request.key):
                    if previous["binding"] != binding:
                        raise Refusal("Identity conflict; preserve the original binding")
                    return previous
            for previous in snapshot["records"]:
                original = json.loads(previous["binding"])
                if any(original[field] == getattr(request, field)
                       for field in ("request_id", "run", "attempt", "effect", "authority")):
                    raise Refusal("Identity conflict; use fresh identities and fresh authority")
            if any(unresolved(previous) for previous in snapshot["records"]):
                raise Refusal("Workspace unresolved; admit independent evidence")
            at("before_intent")
            record = dict(caller=caller, key=request.key, binding=binding, stage="intent",
                          effect="not_executed", observed_subject=None, verified_subject=None,
                          verification=None, receipt=None,
                          reserved_dispatches=0, ruling=None)
            validate_record(record, snapshot["enrollment"])
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
            at("before_seal")
            if terminal == "completed" and not self.verification_current(record, request):
                terminal, reason = "blocked", "stale_verification"
            record.update(stage="published", receipt={"status": terminal, "reason": reason,
                                                      "effect": record["effect"]})
            publish(self.root, snapshot, receipt_cut=at)
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
        record.update(stage="dispatch", effect="unknown", reserved_dispatches=1)
        publish(self.root, snapshot)
        self.target.open_interval(binding)
        at("before_effect")
        refusal = self.gate(binding, request)
        if refusal or script == "cancel_before_effect":
            witness = self.target.fence(binding)
            if not self.valid_witness(witness, binding, request):
                return "failed", "unknown_outcome"
            record.update(stage="observed", effect=witness["outcome"])
            publish(self.root, snapshot)
            return ("blocked", refusal) if refusal else ("interrupted", "cancelled")
        report = self.target.invoke(binding, request, script == "executor_failure")
        at("after_effect")
        # An executor reply alone can't prove what changed. The live witness may be unavailable.
        witness = None if script == "unknown_report" else self.target.fence(binding)
        if not self.valid_witness(witness, binding, request):
            return "failed", "unknown_outcome"
        outcome = witness["outcome"]
        record.update(stage="observed", effect=outcome, observed_subject=witness["output_subject"])
        publish(self.root, snapshot)
        if outcome != "executed" or report != "executed":
            return "failed", "executor_failure"
        if script == "cancel_after_effect":
            return "interrupted", "cancelled"
        at("during_verification")
        record["verified_subject"] = digest(self.target.content)
        passed = script != "verification_failure" and self.verification_current(record, request)
        record.update(stage="verified", verification="passed" if passed else "failed")
        publish(self.root, snapshot)
        return ("completed", "verified") if passed else ("blocked", "verification_failed")

    def verification_current(self, record, request):
        """Require the trusted verifier and the same observed, verified and current output."""
        return (request.verifier == self.verifier and record["observed_subject"]
                == record["verified_subject"] == digest(self.target.content))

    def valid_witness(self, witness, binding, request):
        """Compare enrollment, invocation and committed output against the trusted final record."""
        if not witness or witness != self.target.witnesses.get(binding):
            return False
        if witness.get("binding") != binding:
            return False
        if any(witness.get(key) != value for key, value in self.target.enrollment.items()):
            return False
        if "output_subject" not in witness:
            return False
        if witness.get("outcome") == "executed":
            return witness["output_subject"] == digest(request.replacement)
        return witness.get("outcome") in ("failed", "not_executed") and witness["output_subject"] is None

    def admit(self, actor, caller, request, witness):
        """Append one host ruling for an unresolved invocation after its target fences it."""
        if actor != "host-reconciler":
            raise Refusal("Only the host reconciler may admit evidence")
        binding = request.binding(caller, self.target.enrollment)
        with self.writer() as snapshot:
            record = next((entry for entry in snapshot["records"] if entry["binding"] == binding),
                          None)
            if record is None or record["effect"] != "unknown":
                raise Refusal("Only an unresolved dispatch can receive evidence")
            if not self.valid_witness(witness, binding, request):
                raise Refusal("Supply a final independent witness for this invocation")
            if "dispute" in record:
                raise Refusal("Conflicting evidence; preserve the store for investigation")
            ruling = {"actor": actor, **witness}
            if record["ruling"] is None:
                record["ruling"] = ruling
                publish(self.root, snapshot)
            elif ruling != record["ruling"]:
                record["dispute"] = ruling
                publish(self.root, snapshot)
                raise Refusal("Conflicting evidence; preserve the store for investigation")
            return record

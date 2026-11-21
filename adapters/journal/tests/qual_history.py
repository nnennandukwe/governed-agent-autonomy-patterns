"""Build qualification histories around real, separately observed reference submissions."""

import copy
import ctypes
import json
import os
from pathlib import Path
import select
import struct
import subprocess
import sys
import time
import uuid

from qual_host import LIMITS, canonical, digest, identity, observe_tree, snapshot, write_private

# Use the shipping reference's own known-good input.
sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "examples/submitted-proposal/tests"))
from test_protocol import GOOD

ZERO_USAGE = dict(cost_micros=0, model_tokens=0, elapsed_ms=0, tool_calls=0)


def reference(executable, root, command, value=None):
    """Read official CLI output with a bounded wait."""
    response = subprocess.run([executable, command, root], capture_output=True, check=True,
                              input=canonical(value) if value is not None else None, timeout=20)
    return json.loads(response.stdout)


def number_canonical(value):
    """Use ECMAScript number encoding for the reference's floating-point test report."""
    script = """
const fs = require('fs');
const order = value => Array.isArray(value) ? value.map(order)
  : value && typeof value === 'object'
  ? Object.fromEntries(Object.keys(value).sort().map(key => [key, order(value[key])]))
  : value;
process.stdout.write(JSON.stringify(order(JSON.parse(fs.readFileSync(0, 'utf8')))));
"""
    return subprocess.run(["node", "-e", script], input=canonical(value),
                          capture_output=True, check=True, timeout=10).stdout


def observed_submission(executable, root, proposal, lose_response, trace_prefix=()):
    """Watch this fixture's sole writer rename the target and read its state externally."""
    target = root / "workspace" / proposal["path"]
    before = (identity(target), digest(target.read_bytes()))
    libc = ctypes.CDLL(None, use_errno=True)
    moved_to = 0x80
    fd = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
    if fd < 0:
        raise OSError(ctypes.get_errno(), "inotify_init1")
    try:
        if libc.inotify_add_watch(fd, os.fsencode(target.parent), moved_to) < 0:
            raise OSError(ctypes.get_errno(), "inotify_add_watch")
        checked_at = time.time_ns() // 1_000_000
        child = subprocess.Popen([*trace_prefix, executable, "coding-submit", root], stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, pipesize=4096)
        try:
            child.stdin.write(canonical(proposal))
            child.stdin.close()
            child.stdin = None
            if lose_response:
                deadline = time.monotonic() + 12
                while time.monotonic() < deadline:
                    saved = reference(executable, root, "coding-inspect")
                    if saved["runs"]:
                        break
                    time.sleep(.015)
                else:
                    raise AssertionError("reference didn't publish before the deadline")
                if child.poll() is not None:
                    raise AssertionError("response wasn't interrupted before complete delivery")
                child.kill()
                partial, _ = child.communicate(timeout=5)
                if not partial:
                    raise AssertionError("fixture never started delivering the response")
                try:
                    json.loads(partial)
                except (UnicodeDecodeError, json.JSONDecodeError):
                    pass
                else:
                    raise AssertionError("client received the full response")
                saved_run = saved["runs"][-1]
            else:
                out, err = child.communicate(timeout=12)
                if child.returncode:
                    raise AssertionError(err)
                saved_run = json.loads(out)
        finally:
            if child.poll() is None:
                child.kill()
                child.communicate()
        if not select.select([fd], [], [], 2)[0]:
            raise AssertionError("no observed target rename")
        events = os.read(fd, 8192)
        targets = []
        header = struct.Struct("iIII")
        while events:
            _, mask, _, size = header.unpack_from(events)
            if mask & moved_to:
                targets.append(events[header.size:header.size+size].rstrip(b"\0").decode())
            events = events[header.size+size:]
        if targets.count(target.name) != 1:
            raise AssertionError(("unexpected target replacements", targets))
        after = (identity(target), digest(target.read_bytes()))
        if before[0]["ino"] == after[0]["ino"] or target.read_text() != proposal["content"]:
            raise AssertionError("target observation doesn't match the submitted replacement")
        if lose_response:
            duplicate = reference(executable, root, "coding-submit", proposal)
            if not duplicate["duplicate"] or duplicate["receipt"] != saved_run["receipt"]:
                raise AssertionError("reference failed to recover the original receipt")
            if (identity(target), digest(target.read_bytes())) != after:
                raise AssertionError("reference duplicate changed the target")
            if select.select([fd], [], [], 0)[0]:
                raise AssertionError("duplicate produced an unexpected move")
        return saved_run, before, after, checked_at
    finally:
        os.close(fd)


def reference_profile(run):
    """Pin identities from a separately launched reference and retain them outside the store."""
    request, effect = run["request"], run["effect_request"]
    return {"format": "gaap.local-file-history/0.1", "policies": request["policies"],
            "capability": request["requested_capability"], "tool_schema_digest": effect["tool_schema_digest"],
            "sandbox": effect["sandbox_profile"], "executor": run["execution_evidence"]["executor"],
            "implementer_id": "submitted-proposal-reference", "verifier_id": "independent-shipping-oracle",
            "grant_observer": "grant-host", "effect_observer": "file-host", "require_grant": True}


def historical_attempt(slot, key, proposal, run, observations=None):
    """Build a v0.1 test envelope. The frozen objects are the original CLI's output."""
    request, effect, receipt = run["request"], run["effect_request"], run["receipt"]
    result = next(iter(run["protected_effect_results"]), None)
    principal, attempt_id = slot.host["principal"], uuid.uuid4().hex
    proposal_digest = digest(canonical(proposal))
    request_digest, effect_digest = digest(canonical(request)), digest(canonical(effect))
    binding = digest(canonical([slot.unused["workspace_id"], slot.unused["store_id"],
                                slot.unused["profile_digest"], principal, key, attempt_id,
                                proposal_digest, request_digest, effect_digest]))
    facts = []

    def add(tag, **payload):
        """Append exactly one typed fact to this attempt."""
        facts.append(dict(tag=tag, binding=binding, payload=payload))

    def archive(actor_id, locator, data):
        """Retain the bytes outside the journal under the trusted host's control."""
        entry = {"actor_id": actor_id, "locator": locator, "bytes_hex": data.hex()}
        if entry not in slot.host["archive"]["entries"]:
            slot.host["archive"]["entries"].append(entry)
        return digest(data)

    def observe(actor, kind, locator, data):
        """Create a test-only host evidence binding for an independently supplied observation."""
        hashed = archive(actor, locator, canonical(data))
        return {"actor_id": actor, "evidence": {"evidence_type": kind, "locator": locator, "digest": hashed}}

    add("intent", run_request_digest=request_digest, effect_request_digest=effect_digest,
        proposed_artifact_digest=digest(proposal["content"].encode()))
    if observations is not None:
        before_file, after_file, checked_at = observations
        profile = slot.host["profile"]
        body = result["body"]
        before = {"file": before_file[0], "artifact_digest": before_file[1], "subject": effect["subject"]}
        after = {"file": after_file[0], "artifact_digest": after_file[1], "subject": body["observed_post_effect_subject"]}
        events = receipt["body"]["events"]
        decisions = []
        for event in events:
            if event["event_type"] == "protected_effect_decision":
                decisions.append(event)
                if event["decision"]["code"] == "workflow.mutation_authorized":
                    break
        grants = []
        for approval in request["approval_context"] + effect["approval_context"]:
            actor = profile["grant_observer"]
            source = observe(actor, "approval", "fixture:grant/" + attempt_id,
                             dict(kind="grant", actor_id=actor, binding=binding,
                                  checked_at_ms=checked_at, approval=approval, valid=True))
            grants.append(dict(approval=approval, source=source))
        add("dispatch", before=before, executor=body["executor"], decisions=decisions, checks=grants,
            checked_at_ms=checked_at, reserved={**ZERO_USAGE, "elapsed_ms": 1, "tool_calls": 1},
            usage_floor=ZERO_USAGE)
        if body["execution_status"] == "unknown_outcome":
            # The outside witness saw a rename. This test journal retains only unresolved evidence.
            archive(body["executor"]["name"], "gaap:record/execution_evidence", canonical(run["execution_evidence"]))
            add("observed", disposition="unresolved", after=None, source=None, usage_floor=body["usage"])
        else:
            actor = profile["effect_observer"]
            source = observe(actor, "artifact", "fixture:effect/" + attempt_id,
                             dict(kind="observation", actor_id=actor, binding=binding, disposition="replaced",
                                  before=before, after=after, covers_through_stop=True))
            add("observed", disposition="replaced", after=after, source=source, usage_floor=body["usage"])
            event = next(event for event in events if event["event_type"] == "verification")
            verifier = profile["verifier_id"]
            archive(verifier, "gaap:record/verification", number_canonical(run["verification"]))
            archive(verifier, "gaap:record/after", canonical(run["execution_evidence"]["after"]))
            archive(verifier, "gaap:journal/verification/" + attempt_id,
                    canonical(dict(kind="verification", actor_id=verifier, binding=binding, event=event)))
            archive(body["executor"]["name"], "gaap:record/execution_evidence", canonical(run["execution_evidence"]))
            add("verified", event=event, usage_floor=receipt["body"]["usage"])
    add("published", receipt=receipt, result=result)
    return {"principal": principal, "key": key, "attempt_id": attempt_id, "proposal": proposal,
            "proposal_digest": proposal_digest, "run_request": request, "effect_request": effect,
            "facts": facts}


def exercise_histories(case, executable, evidence_dir):
    """Use a qualified reader for complete, partial, failed, refused and damaged history."""
    root = case.root / "reference"
    initial = reference(executable, root, "coding-init")
    proposal = dict(path="shipping.py", content=GOOD + "# " + "x"*10000 + "\n",
                    plan="Implement shipping rules.", base_digest=initial["subject_digest"])
    run, *observed = observed_submission(executable, root, proposal, lose_response=True)
    case.assertEqual(run["receipt"]["body"]["terminal_status"], "completed")
    slot = case.slot(workspace=root / "workspace", profile=reference_profile(run))
    slot.call("enroll", expected="enrolled")
    good = historical_attempt(slot, "success", proposal, run, observed)

    # The real verifier rejects this replacement. The change to the file survives.
    failed_proposal = {**proposal, "base_digest": reference(executable, root, "coding-inspect")["subject_digest"],
                       "content": "def shipping_quote(quantity, unit_price):\n    return 9\n"}
    failed_run, *failed_obs = observed_submission(executable, root, failed_proposal, lose_response=False)
    case.assertNotEqual(failed_run["receipt"]["body"]["terminal_status"], "completed")
    failed = historical_attempt(slot, "failure", failed_proposal, failed_run, failed_obs)
    refused_proposal = {**proposal, "path": "production.json", "content": "{}",
                        "base_digest": reference(executable, root, "coding-inspect")["subject_digest"]}
    expected_tree = observe_tree(slot.workspace)
    refused_run = reference(executable, root, "coding-submit", refused_proposal)
    case.assertNotEqual(refused_run["receipt"]["body"]["terminal_status"], "completed")
    case.assertEqual(observe_tree(slot.workspace), expected_tree)
    refused = historical_attempt(slot, "refused", refused_proposal, refused_run)
    slot.configure()
    # Retain the actual enrollment anchor while the fixture producer publishes later facts.
    empty = slot.journal.read_bytes()
    for generation, status, reason in [(1, "unconfirmed", "journal_ahead_of_anchor"),
                                       (2, "invalid_store", "journal_confirmation_mismatch")]:
        prefix = {**good, "facts": good["facts"][:generation]}
        ahead = {**json.loads(empty), "generation": generation, "attempts": [prefix]}
        write_private(slot.journal, canonical(ahead))
        retained = snapshot(slot.anchor), snapshot(slot.journal)
        failure = slot.call("lookup", "success", expected=status)
        case.assertEqual(failure["reason"], reason)
        case.assertNotIn("attempt", failure)
        case.assertEqual((snapshot(slot.anchor), snapshot(slot.journal)), retained)
        case.record(slot, "populated-generation-gap-" + str(generation), failure)
    attempts = [good, failed, refused]
    slot.install_history(attempts)
    # Preserve the actual frozen tuples, retained witness bytes and independent inode/hash observations.
    # All content is from the disposable shipping fixture. Restore isn't an adapter operation.
    (evidence_dir / "populated-fixture.json").write_bytes(canonical({
        "host": slot.host, "anchor": json.loads(slot.anchor.read_bytes()),
        "journal": json.loads(slot.journal.read_bytes()),
        "observed_replacements": [
            {"key": "success", "before": observed[0], "after": observed[1]},
            {"key": "failure", "before": failed_obs[0], "after": failed_obs[1]}]}))
    for attempt in attempts:
        reply = slot.call("lookup", attempt["key"], expected="published")
        case.assertEqual(reply["attempt"]["facts"], attempt["facts"])
        case.record(slot, "retained-" + attempt["key"], {"status": reply["status"],
                    "receipt": attempt["facts"][-1]["payload"]["receipt"]["receipt_digest"]})

    saved = snapshot(slot.anchor), snapshot(slot.journal)
    slot.call("lookup", "success", "--proposal-stdin", expected="published",
              stdin=json.dumps(proposal, ensure_ascii=False, indent=2).encode())
    slot.call("lookup", "success", "--proposal-stdin", expected="conflict",
              stdin=canonical({**proposal, "plan": "Different request."}))
    boundary_input = canonical(proposal).ljust(LIMITS["raw_proposal_bytes"], b" ")
    slot.call("lookup", "success", "--proposal-stdin", expected="published", stdin=boundary_input)
    slot.call("lookup", "success", "--proposal-stdin", expected="capacity", stdin=boundary_input + b" ")
    slot.host["principal"] = "other"
    slot.configure()
    slot.call("lookup", "success", expected="absent")
    slot.host["principal"] = "alice"
    slot.configure()
    case.assertEqual((snapshot(slot.anchor), snapshot(slot.journal)), saved)

    for count in range(1, 5):
        prefix = {**good, "facts": good["facts"][:count]}
        slot.install_history([prefix])
        previous = snapshot(slot.journal)
        reply = slot.call("lookup", "success", expected="partial")
        case.assertEqual(reply["attempt"]["facts"], prefix["facts"])
        if count == 2:
            case.assertTrue(reply["workspace_hold"])
            case.assertTrue(slot.call("lookup", "never", expected="absent")["workspace_hold"])
            # Current target bytes can't settle a prior dispatch on either side of its rename.
            target = slot.workspace / "shipping.py"
            for physical in (run["before"], proposal["content"]):
                target.write_text(physical)
                observed_file = snapshot(target)
                duplicate = slot.call("lookup", "success", expected="partial")
                case.assertTrue(duplicate["workspace_hold"])
                case.assertEqual(duplicate["attempt"], reply["attempt"])
                case.assertEqual(snapshot(target), observed_file)
            target.write_text(failed_proposal["content"])
        case.assertEqual(snapshot(slot.journal), previous)
        case.record(slot, "retained-prefix-" + str(count), {"status": reply["status"], "hold": reply["workspace_hold"]})
    case.assertEqual(observe_tree(slot.workspace), expected_tree)

    # Fill the retained-attempt limit with twelve distinct, real request/result pairs.
    while len(attempts) < LIMITS["attempts"]:
        current = dict(refused_proposal, plan=f"Denied request {len(attempts) + 1}.")
        blocked = reference(executable, root, "coding-submit", current)
        attempts.append(historical_attempt(slot, f"refused-{len(attempts)}", current, blocked))
    slot.install_history(attempts)
    full_store = snapshot(slot.journal)
    slot.call("lookup", attempts[-1]["key"], expected="published")
    case.assertEqual(snapshot(slot.journal), full_store)
    case.record(slot, "full-readable-store", {"attempts": len(attempts)})
    wrong = copy.deepcopy(good)
    wrong["facts"][-1]["payload"]["result"] = failed["facts"][-1]["payload"]["result"]
    for histories in ([good, good], [failed, wrong], [good] * (12 + 1)):
        slot.install_history(histories)
        previous = snapshot(slot.journal)
        slot.call("lookup", "missing", expected=("invalid_store", "capacity"))
        case.assertEqual(snapshot(slot.journal), previous)

def exercise_unknown(case, executable, evidence_dir):
    """Return an actual unknown-effect receipt after injected post-rename fsync failure."""
    root = case.root / "fault-reference"
    initial = reference(executable, root, "coding-init")
    proposal = dict(path="shipping.py", content=GOOD, plan="Qualify the reference sync.",
                    base_digest=initial["subject_digest"])
    baseline_output = evidence_dir / "reference-sync-order.log"
    trace = ["strace", "-qq", "-yy", "-e", "trace=fsync"]
    healthy, *_ = observed_submission(executable, root, proposal, False,
                                      [*trace, "-o", str(baseline_output)])
    volume_fd = str(root / "workspace") + ">"
    syncs = [l for l in baseline_output.read_text().splitlines() if l.startswith("fsync(")]
    targets = [index for index, line in enumerate(syncs, start=1) if volume_fd in line]
    case.assertEqual(len(targets), 1, syncs)
    fault_output = evidence_dir / "reference-failed-directory-sync.log"
    proposal = {**proposal, "content": GOOD + "# second recorded invocation\n",
                "base_digest": reference(executable, root, "coding-inspect")["subject_digest"]}
    run, *observed = observed_submission(
        executable, root, proposal, False,
        [*trace, "-o", str(fault_output), "-e", f"inject=fsync:error=EIO:when={targets[0]}"])
    case.assertTrue(any(volume_fd in line and "EIO" in line and "INJECTED" in line
                        for line in fault_output.read_text().splitlines()))
    case.assertEqual(run["protected_effect_results"][0]["body"]["execution_status"], "unknown_outcome")
    case.assertNotEqual(run["receipt"]["body"]["terminal_status"], "completed")
    slot = case.slot(workspace=root / "workspace", profile=reference_profile(healthy))
    slot.call("enroll", expected="enrolled")
    attempt = historical_attempt(slot, "uncertain", proposal, run, observed)
    slot.configure()
    slot.install_history([attempt])
    before = snapshot(slot.journal), observe_tree(slot.workspace)
    reply = slot.call("lookup", "uncertain", expected="published")
    case.assertTrue(reply["workspace_hold"])
    case.assertEqual(reply["attempt"]["facts"][-1]["payload"]["receipt"], run["receipt"])
    case.assertTrue(slot.call("lookup", "other-key", expected="absent")["workspace_hold"])
    case.assertEqual((snapshot(slot.journal), observe_tree(slot.workspace)), before)
    case.record(slot, "sealed-unresolved", {"hold": True, "receipt": run["receipt"]["receipt_digest"],
                "observed_before": observed[0], "observed_after": observed[1],
                "sealed_result": run["protected_effect_results"][0]})

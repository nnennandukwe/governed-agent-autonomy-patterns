"""Record public rehearsal controls and verify them against a separately retained digest."""

import argparse
from collections import Counter
from hashlib import sha256
import json
import os
from pathlib import Path
import platform
import stat
import tempfile
import time

import corpus


HERE = Path(__file__).resolve().parent
ARMS = ("raw", "revision", "lesson", "resampling")
RESOURCES = ("provider_calls", "total_tokens", "candidates", "strategy_tool_calls",
             "evaluator_calls", "subagent_calls")
MODE = "offline_development_controls"
OPERATIONS = {"initial_check": "strategy_tool_calls", "response": "candidates",
              "public_check": "strategy_tool_calls", "placement": "strategy_tool_calls",
              "score": "evaluator_calls"}


def encoded(value):
    """Return stable UTF-8 JSON bytes for host-owned records."""
    return (json.dumps(value, sort_keys=True, ensure_ascii=False, indent=2) + "\n").encode()


def save(path, value):
    """Replace one host record atomically, including its last pre-effect checkpoint."""
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as staged:
        staged.write(encoded(value))
    os.replace(staged.name, path)


def read(path):
    """Decode a host-owned JSON file."""
    return json.loads(path.read_bytes())


def source_hashes():
    """Bind the exact executable, independent evaluator, inputs, pricing and arm instructions."""
    inputs = ["recorder.py", "corpus.py", "budget.json", "study.json", "split.json"]
    inputs += sorted(str(path.relative_to(HERE)) for path in (HERE / "fixtures").glob("*.json"))
    return {name: sha256((HERE / name).read_bytes()).hexdigest() for name in inputs}


def assignment_manifest():
    """List the proposed frozen cells and identify which ones the rehearsal exercises locally."""
    corpus.verify()
    study = read(HERE / "study.json")
    budget = read(HERE / "budget.json")
    if tuple(arm["id"] for arm in study["arms"]) != ARMS:
        raise ValueError("The frozen arm definitions don't match the recorder.")
    if budget["authorization"] != {"provider_calls": 0, "provider_cost_microusd": 0}:
        raise ValueError("This recorder accepts only zero provider authorization.")
    assignments = [
        {"split": split, "member": member, "arm": arm, "status": "unattempted"}
        for split, member in corpus.members() for arm in ARMS
    ]
    if len(assignments) != budget["study_limits"]["trials"]:
        raise ValueError("The proposed assignment count differs from the study budget.")
    return {"format": "feedback-repair-rehearsal/1", "mode": MODE,
            "sources_sha256": source_hashes(), "provider_assignments": assignments}


def fixture_response(replacement, claimed_complete, lesson=None):
    """Encode the explicit claim and replacement from a scripted public control."""
    return encoded({"replacement_utf8": replacement.decode(), "claimed_complete": claimed_complete,
                    "refusal": None, "lesson": lesson})


def script(arm, family_index, gold, decoy):
    """Supply inert fixture replies without consulting the final evaluator."""
    good = fixture_response(gold, True)
    failed = fixture_response(b"{}", False)
    if arm == "raw":
        return [good] if family_index == 0 else []
    if arm == "resampling":
        return [fixture_response(decoy, True) if family_index == 0 else failed, good]
    if arm == "lesson":
        return [failed, fixture_response(gold, True, "Restore the field missing from the public check.")]
    return [failed, good]


def parse_reply(raw, lesson_required):
    """Read the four frozen response fields without inferring a completion claim."""
    data = corpus.decode(raw, limit=corpus.MAX_BUNDLE_BYTES)
    if not isinstance(data, dict) or set(data) != {"replacement_utf8", "claimed_complete", "refusal", "lesson"}:
        raise ValueError("Unexpected response fields.")
    replacement, refusal, claim, lesson = (data[key] for key in
                                           ("replacement_utf8", "refusal", "claimed_complete", "lesson"))
    if type(claim) is not bool:
        raise ValueError("A response needs an explicit Boolean completion claim.")
    if replacement is None:
        if not isinstance(refusal, str) or not refusal.strip() or claim:
            raise ValueError("A refusal needs a reason and an unclaimed result.")
    elif not isinstance(replacement, str) or refusal is not None:
        raise ValueError("A replacement must be UTF-8 text without a refusal.")
    if lesson_required and (replacement is not None or lesson is not None):
        if not isinstance(lesson, str) or not lesson.strip():
            raise ValueError("A lesson-arm revision needs a lesson.")
    elif lesson is not None:
        raise ValueError("Only a lesson-arm revision can carry a lesson.")
    return data


def common_request(study, task, workspace, initial_check):
    """Assemble the same public input for all first calls and each independent sample."""
    return {"prompt": study["base_prompt"], "target": task["target"], "initial_public_pass": initial_check,
            "files": {p.name: p.read_bytes().decode() for p in sorted(workspace.iterdir())}}


def request_for(study, initial, arm, prior):
    """Expose only the first request or the failed parent allowed by this arm."""
    if prior is None:
        return initial
    instruction = next(item["prompt"] for item in study["arms"] if item["id"] == arm)
    return dict(initial, instruction=instruction, previous_reply_hex=prior["reply_hex"],
                previous_public_pass=prior["public_pass"])


def bound_lesson(task, number, previous, text):
    """Attribute one scripted lesson to this member and its observed failed parent."""
    return {"member": task["id"], "arm": "lesson", "failed_candidate": number,
            "sha256": previous["sha256"], "public_pass": previous["public_pass"],
            "public_checks": task["checks"], "text": text}


def decode_candidate(reply, lesson_required):
    """Extract the replayable response fields and retain a reason if no proposal arrived."""
    try:
        parsed = parse_reply(reply, lesson_required)
    except (ValueError, UnicodeError) as exc:
        return {}, "malformed_response", str(exc), None
    fields = {"claimed_complete": parsed["claimed_complete"]}
    if parsed["refusal"] is not None:
        return fields, "refused", parsed["refusal"], parsed["lesson"]
    replacement = parsed["replacement_utf8"].encode()
    if len(replacement) > corpus.MAX_REPLACEMENT_BYTES:
        return fields, "malformed_response", "The replacement exceeds the size limit.", None
    fields.update(sha256=sha256(replacement).hexdigest(), utf8=parsed["replacement_utf8"])
    return fields, None, None, parsed["lesson"]


def stop_after_check(arm, public_pass):
    """Close raw sampling after one proposal and revision arms after a public pass."""
    return arm == "raw" or (arm in ("revision", "lesson") and public_pass)


def select(arm, candidates):
    """Choose from proposals that received public checks, excluding replies that refused."""
    eligible = [i for i, item in enumerate(candidates) if type(item["public_pass"]) is bool]
    if not eligible:
        return None
    if arm == "resampling":
        return next((i for i in eligible if candidates[i]["public_pass"]), eligible[0])
    if arm in ("revision", "lesson"):
        if len(candidates) == 1 and not candidates[0]["public_pass"]:
            return None
        return eligible[-1]
    return eligible[0]


class LimitReached(Exception):
    """Stop the current control before the next charged operation."""


def run_control(task, proposals, arm, limits, record_path):
    """Run one development control, retain the selection before its single possible write."""
    study = read(HERE / "study.json")
    started = time.monotonic_ns()
    usage = dict.fromkeys(RESOURCES, 0)
    record = {"mode": MODE, "member": task["id"], "arm": arm, "status": "started",
              "usage": usage, "operations": [], "candidates": [], "selected": None, "effect": "none",
              "evaluation": None, "before": None, "after": None, "lesson": None, "initial_request_utf8": None}
    save(record_path, record)

    def charge(operation="finish"):
        """Check the monotonic deadline and reserve one operation before dispatch."""
        if time.monotonic_ns() - started >= limits["elapsed_seconds"] * 1_000_000_000:
            record["stopped_before"] = operation
            raise LimitReached("elapsed_seconds")
        if operation != "finish":
            resource = OPERATIONS[operation]
            if usage[resource] >= limits[resource]:
                record["stopped_before"] = operation
                raise LimitReached(resource)
            usage[resource] += 1
            record["operations"].append(operation)

    with tempfile.TemporaryDirectory() as name:
        parent = Path(name)
        subject = parent / "selected"
        record["before"] = corpus.reset(task, subject)
        try:
            charge("initial_check")
            initial_pass = corpus.public_checks(task, (subject / task["target"]).read_bytes())
            initial = common_request(study, task, subject, initial_pass)
            record["initial_request_utf8"] = encoded(initial).decode()
            for number, reply in enumerate(proposals):
                previous = record["candidates"][-1] if number else None
                revised = previous is not None and arm in ("revision", "lesson")
                charge("response")
                if corpus.reset(task, parent / f"proposal-{number}") != record["before"]:
                    raise ValueError("A proposal started from different input bytes.")
                request = request_for(study, initial, arm, previous if revised else None)
                candidate = {"request_utf8": encoded(request).decode(), "reply_hex": reply.hex(),
                             "parent": number - 1 if revised else None, "public_pass": None}
                record["candidates"].append(candidate)
                fields, status, reason, lesson = decode_candidate(reply, revised and arm == "lesson")
                candidate.update(fields)
                if lesson is not None:
                    record["lesson"] = bound_lesson(task, number - 1, previous, lesson)
                if status is not None:
                    if status == "refused" and revised:
                        break
                    record.update(status=status, reason=reason)
                    break
                replacement = fields["utf8"].encode()
                charge("public_check")
                candidate["public_pass"] = corpus.public_checks(task, replacement)
                if stop_after_check(arm, candidate["public_pass"]):
                    break
            stopped = record["status"] in ("malformed_response", "refused")
            chosen = None if stopped else select(arm, record["candidates"])
            record["selected"] = chosen
            if chosen is None:
                if not stopped:
                    record["status"] = "missing_proposal"
            else:
                replacement = record["candidates"][chosen]["utf8"].encode()
                record["status"] = "selected"
                save(record_path, record)
                charge("placement")
                record["effect"] = "uncertain"
                save(record_path, record)
                with tempfile.NamedTemporaryFile(dir=subject, delete=False) as stage:
                    stage.write(replacement)
                os.replace(stage.name, subject / task["target"])
                record["after"] = corpus.snapshot(subject)
                if record["after"].get(task["target"]) != sha256(replacement).hexdigest():
                    raise ValueError("The observed target differs from the selected bytes.")
                record["effect"] = "observed"
                charge("score")
                result = corpus.evaluate(task, record["before"], subject, replacement)
                record["evaluation"] = result
                record["status"] = "verified" if result["effect_valid"] and result["correct"] else "rejected"
            charge()
        except LimitReached as exc:
            record.update(status="timeout" if str(exc) == "elapsed_seconds" else "exhausted",
                          reason=str(exc))
        except (OSError, UnicodeError, ValueError) as exc:
            record.update(status="unresolved" if record["effect"] == "uncertain" else "error",
                          reason=str(exc))
        if record["after"] is None and record["effect"] == "none":
            record["after"] = corpus.snapshot(subject)
    usage["elapsed_ns"] = time.monotonic_ns() - started
    if usage["elapsed_ns"] >= limits["elapsed_seconds"] * 1_000_000_000 and record["status"] in (
        "verified", "rejected", "missing_proposal"
    ):
        record.update(status="timeout", reason="elapsed_seconds", stopped_before="finish")
    save(record_path, record)
    if record["effect"] == "uncertain":
        raise ValueError(f"Uncertain effect at {record_path.name}. Preserve this incomplete packet.")
    return record


def summary(manifest, records):
    """Keep study denominators apart from actual local outcomes and charged operations."""
    assignments = manifest["provider_assignments"]
    counts = Counter(record["status"] for record in records)
    zeros = dict.fromkeys(RESOURCES, 0)
    selected = [record["candidates"][record["selected"]] if record["selected"] is not None else {}
                for record in records]
    claimed = [record for candidate, record in zip(selected, records) if candidate.get("claimed_complete") is True]
    failures = sum(record["status"] == "rejected" for record in claimed)
    claim_counts = {"claimed": len(claimed),
                    "unclaimed": sum(item.get("claimed_complete") is False for item in selected),
                    "unknown": sum("claimed_complete" not in item for item in selected),
                    "unresolved": sum(record["status"] not in ("verified", "rejected") for record in claimed),
                    "false_completions": failures, "per_attempt": failures / len(records) if records else None,
                    "per_claim": failures / len(claimed) if claimed else None}
    scored = sum(record["status"] in ("verified", "rejected") for record in records)
    totals = {key: sum(record["usage"][key] for record in records) for key in (*RESOURCES, "elapsed_ns")}
    return {
        "mode": MODE,
        "provider_study": {"assigned": len(assignments), "attempted": 0, "unattempted": len(assignments),
                           "assigned_by_split": dict(Counter(item["split"] for item in assignments)),
                           "verified_rate": None, "accrued_usage": zeros,
                           "accrued_provider_cost_microusd": 0, "trial_cost_microusd": None},
        "development_controls": {"assigned": len(records), "outcomes": dict(counts),
                                 "scripted_completion_claims": claim_counts,
                                 "scored": scored, "unscored": len(records) - scored,
                                 "retained_late_evaluations": sum(record["status"] == "timeout" and
                                                                  record["evaluation"] is not None for record in records),
                                 "usage": totals, "provider_cost_microusd": 0},
    }


def packet_bytes(directory, names):
    """Read exactly the expected regular files once, with a per-entry byte ceiling."""
    expected = set(names)
    for entry in directory.iterdir():
        if entry.name not in expected:
            raise ValueError("Unexpected or missing packet files. Preserve this incomplete packet.")
        expected.remove(entry.name)
    if expected:
        raise ValueError("Unexpected or missing packet files. Preserve this incomplete packet.")
    contents = {}
    for name in names:
        flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
        with os.fdopen(os.open(directory / name, flags), "rb") as entry:
            if not stat.S_ISREG(os.fstat(entry.fileno()).st_mode):
                raise ValueError(f"Packet entry must be a regular file: {name}")
            data = entry.read(1_048_577)
        if len(data) > 1_048_576:
            raise ValueError(f"Packet entry exceeds 1 MiB: {name}")
        contents[name] = data
    return contents


def root_digest(contents):
    """Commit the exact names and bytes that the verifier parses."""
    digests = {name: sha256(data).hexdigest() for name, data in contents.items()}
    return sha256(encoded(digests)).hexdigest()


def local_controls():
    """Pair two public development families with the four proposed arm instructions."""
    for family_index, (task, gold, decoy) in enumerate(corpus.controls()):
        for arm in ARMS:
            yield task, arm, script(arm, family_index, gold, decoy)


def rehearse(directory):
    """Create a new packet and return its separately retainable root only after completion."""
    manifest = assignment_manifest()
    limits = read(HERE / "budget.json")["per_cell_limits"]
    directory.mkdir(mode=0o700)
    manifest["runtime"] = {"python": platform.python_version(), "implementation": platform.python_implementation(),
                           "system": platform.system(), "release": platform.release(), "machine": platform.machine()}
    save(directory / "manifest.json", manifest)
    records = []
    names = ["manifest.json", "summary.json"]
    for index, (task, arm, proposals) in enumerate(local_controls()):
        name = f"control-{index}.json"
        records.append(run_control(task, proposals, arm, limits, directory / name))
        names.append(name)
    save(directory / "summary.json", summary(manifest, records))
    return root_digest(packet_bytes(directory, names))


def verified_history(task, arm, proposals, record, initial):
    """Rebuild each arrived reply and public observation, including the final partial step."""
    candidates = record["candidates"]
    if record["initial_request_utf8"] is None:
        if candidates:
            raise ValueError("A proposal has no starting request.")
        return [], "initial_check", None, None
    if record["initial_request_utf8"] != encoded(initial).decode():
        raise ValueError("The initial request differs from the frozen subject.")
    ops = ["initial_check"]
    study = read(HERE / "study.json")
    lesson = None
    next_op = "response"
    stop = None
    for number, candidate in enumerate(candidates):
        if next_op != "response" or number >= len(proposals):
            raise ValueError("Unexpected response after the strategy stopped.")
        parent = number - 1 if number and arm in ("revision", "lesson") else None
        previous = candidates[parent] if parent is not None else None
        reply = proposals[number]
        fields, status, reason, text = decode_candidate(reply, previous is not None and arm == "lesson")
        checked = candidate["public_pass"]
        expected = dict(fields, reply_hex=reply.hex(), request_utf8=encoded(
            request_for(study, initial, arm, previous)).decode(), parent=parent, public_pass=checked)
        if candidate != expected or (fields.get("claimed_complete") is not candidate.get("claimed_complete")):
            raise ValueError("Candidate bytes, claims or parent differ from the scripted response.")
        ops.append("response")
        if text is not None:
            lesson = bound_lesson(task, parent, previous, text)
        if checked is not None:
            if status is not None or type(checked) is not bool or checked != corpus.public_checks(task, fields["utf8"].encode()):
                raise ValueError("A recorded public check differs from its replacement.")
            ops.append("public_check")
        if status:
            if not (status == "refused" and previous is not None):
                stop = (status, reason)
            next_op = "finish" if stop else "placement"
        elif checked is None:
            next_op = "public_check"
        elif number + 1 == len(proposals) or stop_after_check(arm, checked):
            next_op = "placement"
    if record["lesson"] != lesson:
        raise ValueError("The lesson differs from the recorded failure and reply.")
    if not proposals:
        next_op = "finish"
    chosen = None
    if next_op == "placement":
        chosen = select(arm, candidates)
        if chosen is None:
            next_op = "finish"
    return ops, next_op, chosen, stop


def verify_control(task, arm, proposals, record, limits):
    """Verify all observed operations of one terminal control in a disposable workspace."""
    status, usage = record["status"], record["usage"]
    finished = ("verified", "rejected", "missing_proposal")
    stopped = ("timeout", "exhausted", "malformed_response", "refused", "error")
    if status not in finished + stopped or record["effect"] not in ("none", "observed"):
        raise ValueError("Incomplete control. Preserve this packet.")
    if status in stopped and not record.get("reason"):
        raise ValueError("A stopped control needs its recorded reason.")
    if set(usage) != {*RESOURCES, "elapsed_ns"} or any(type(n) is not int or n < 0 for n in usage.values()):
        raise ValueError("Resource counts and elapsed nanoseconds must be nonnegative integers.")
    with tempfile.TemporaryDirectory() as name:
        scratch = Path(name) / "verification"
        before = corpus.reset(task, scratch)
        initial_check = corpus.public_checks(task, (scratch / task["target"]).read_bytes())
        initial = common_request(read(HERE / "study.json"), task, scratch, initial_check)
        ops, next_op, chosen, response_stop = verified_history(task, arm, proposals, record, initial)
        if record["selected"] != chosen or (chosen is not None and type(record["selected"]) is not int):
            raise ValueError("Selection doesn't follow the frozen public rule.")
        expected_status = "missing_proposal" if response_stop is None else response_stop[0]
        if record["effect"] == "observed":
            if chosen is None:
                raise ValueError("A recorded write needs a selected replacement.")
            selected = record["candidates"][chosen]["utf8"].encode()
            (scratch / task["target"]).write_bytes(selected)
            ops.append("placement")
            next_op = "score"
            if record["evaluation"] is not None:
                result = corpus.evaluate(task, before, scratch, selected)
                if result != record["evaluation"]:
                    raise ValueError("The retained evaluation differs from independent scoring.")
                expected_status = "verified" if result["effect_valid"] and result["correct"] else "rejected"
                ops.append("score")
                next_op = "finish"
        elif record["evaluation"] is not None:
            raise ValueError("Scoring requires an observed effect.")
        if before != record["before"] or corpus.snapshot(scratch) != record["after"]:
            raise ValueError("The retained snapshots differ from the observed prefix.")
    if status == "error" and next_op in OPERATIONS and record["operations"] == ops + [next_op]:
        # An admitted call can raise before producing its first observable result.
        ops.append(next_op)
    if record["operations"] != ops:
        raise ValueError("Operation history differs from the observed prefix.")
    expected_usage = Counter(OPERATIONS[op] for op in ops)
    for key in RESOURCES:
        if usage[key] != expected_usage[key] or usage[key] > limits[key]:
            raise ValueError(f"The {key} charge differs from its observed operations or exceeds its allowance.")
    elapsed_limit = limits["elapsed_seconds"] * 1_000_000_000
    if status in ("timeout", "exhausted"):
        if record.get("stopped_before") != next_op:
            raise ValueError("The stop point disagrees with the observed prefix.")
        if status == "timeout":
            if record["reason"] != "elapsed_seconds" or usage["elapsed_ns"] < elapsed_limit:
                raise ValueError("The recorded time doesn't reach the deadline.")
        else:
            resource = OPERATIONS.get(next_op)
            if resource is None or record["reason"] != resource or usage[resource] != limits[resource]:
                raise ValueError("The stop doesn't exhaust the next operation's allowance.")
    elif status != "error":
        if next_op != "finish" or expected_status != status:
            raise ValueError("Terminal status differs from the observed evidence.")
        if response_stop and record["reason"] != response_stop[1]:
            raise ValueError("A stopped response needs its original reason.")
        if status in finished and usage["elapsed_ns"] >= elapsed_limit:
            raise ValueError("The control completed past its wall-time limit.")


def verify(directory, expected_root):
    """Match a trusted commitment, check current sources, selection lineage and denominators."""
    controls = list(local_controls())
    names = [f"control-{index}.json" for index in range(len(controls))]
    contents = packet_bytes(directory, ["manifest.json", "summary.json", *names])
    if root_digest(contents) != expected_root:
        raise ValueError("Packet bytes differ from the supplied root.")
    manifest = json.loads(contents["manifest.json"])
    expected = assignment_manifest()
    if any(manifest.get(key) != value for key, value in expected.items()):
        raise ValueError("Source bytes or proposed assignments differ from the recording.")
    records = [json.loads(contents[name]) for name in names]
    limits = read(HERE / "budget.json")["per_cell_limits"]
    for (task, arm, proposals), record in zip(controls, records):
        if (record["mode"], record["member"], record["arm"]) != (MODE, task["id"], arm):
            raise ValueError("A control is attributed to the wrong development assignment.")
        verify_control(task, arm, proposals, record, limits)
    checked = summary(manifest, records)
    if checked != json.loads(contents["summary.json"]):
        raise ValueError("The summary differs from the recorded controls.")
    return checked


def main():
    """Run the offline CLI and emit a root or a verified summary on standard output."""
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    prepare = actions.add_parser("rehearse", help="create a packet in a new scratch directory")
    prepare.add_argument("directory", type=Path)
    check = actions.add_parser("verify", help="verify a packet against an independently retained root")
    check.add_argument("directory", type=Path)
    check.add_argument("--expected-root", required=True)
    args = parser.parse_args()
    try:
        if args.action == "rehearse":
            print(rehearse(args.directory))
        else:
            print(json.dumps(verify(args.directory, args.expected_root), sort_keys=True))
    except (OSError, ValueError, KeyError, TypeError) as exc:
        parser.exit(1, f"{parser.prog}: {exc}\n")


if __name__ == "__main__":
    main()

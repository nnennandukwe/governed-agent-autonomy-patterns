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
    if lesson_required:
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


def select(arm, candidates):
    """Close selection in generation order using only completed public observations."""
    if not candidates:
        return None
    if arm == "resampling":
        return next((i for i, item in enumerate(candidates) if item["public_pass"]), 0)
    return len(candidates) - 1 if arm in ("lesson", "revision") else 0


class LimitReached(Exception):
    """Stop the current control before the next charged operation."""


def run_control(task, proposals, arm, limits, record_path):
    """Run one development control, retain the selection before its single possible write."""
    study = read(HERE / "study.json")
    started = time.monotonic_ns()
    usage = dict.fromkeys(RESOURCES, 0)
    record = {"mode": MODE, "member": task["id"], "arm": arm, "status": "started",
              "usage": usage, "candidates": [], "selected": None, "effect": "none",
              "evaluation": None, "before": None, "after": None, "lesson": None, "initial_request_utf8": None}
    save(record_path, record)

    def charge(resource=None):
        """Check the monotonic deadline and reserve one operation before dispatch."""
        if time.monotonic_ns() - started >= limits["elapsed_seconds"] * 1_000_000_000:
            raise LimitReached("elapsed_seconds")
        if resource:
            if usage[resource] >= limits[resource]:
                raise LimitReached(resource)
            usage[resource] += 1

    with tempfile.TemporaryDirectory() as name:
        parent = Path(name)
        subject = parent / "selected"
        record["before"] = corpus.reset(task, subject)
        try:
            charge("strategy_tool_calls")
            initial_pass = corpus.public_checks(task, (subject / task["target"]).read_bytes())
            initial = common_request(study, task, subject, initial_pass)
            record["initial_request_utf8"] = encoded(initial).decode()
            for number, reply in enumerate(proposals):
                previous = record["candidates"][-1] if number else None
                revised = previous is not None and arm in ("revision", "lesson")
                if revised and previous["public_pass"]:
                    break
                charge("candidates")
                if corpus.reset(task, parent / f"proposal-{number}") != record["before"]:
                    raise ValueError("A proposal started from different input bytes.")
                request = request_for(study, initial, arm, previous if revised else None)
                candidate = {"request_utf8": encoded(request).decode(), "reply_hex": reply.hex(),
                             "parent": number - 1 if revised else None, "public_pass": None}
                record["candidates"].append(candidate)
                try:
                    parsed = parse_reply(reply, revised and arm == "lesson")
                except (ValueError, UnicodeError) as exc:
                    record.update(status="malformed_response", reason=str(exc))
                    break
                candidate["claimed_complete"] = parsed["claimed_complete"]
                if parsed["refusal"] is not None:
                    record.update(status="refused", reason=parsed["refusal"])
                    break
                replacement = parsed["replacement_utf8"].encode()
                if len(replacement) > corpus.MAX_REPLACEMENT_BYTES:
                    record.update(status="malformed_response", reason="The replacement exceeds the size limit.")
                    break
                candidate.update(sha256=sha256(replacement).hexdigest(), utf8=parsed["replacement_utf8"])
                if parsed["lesson"] is not None:
                    record["lesson"] = bound_lesson(task, number - 1, previous, parsed["lesson"])
                charge("strategy_tool_calls")
                candidate["public_pass"] = corpus.public_checks(task, replacement)
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
                charge("strategy_tool_calls")
                record["effect"] = "uncertain"
                save(record_path, record)
                with tempfile.NamedTemporaryFile(dir=subject, delete=False) as stage:
                    stage.write(replacement)
                os.replace(stage.name, subject / task["target"])
                record["after"] = corpus.snapshot(subject)
                if record["after"].get(task["target"]) != sha256(replacement).hexdigest():
                    raise ValueError("The observed target differs from the selected bytes.")
                record["effect"] = "observed"
                charge("evaluator_calls")
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
        record.update(status="timeout", reason="elapsed_seconds")
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
    totals = {key: sum(record["usage"][key] for record in records) for key in (*RESOURCES, "elapsed_ns")}
    return {
        "mode": MODE,
        "provider_study": {"assigned": len(assignments), "attempted": 0, "unattempted": len(assignments),
                           "assigned_by_split": dict(Counter(item["split"] for item in assignments)),
                           "verified_rate": None, "accrued_usage": zeros,
                           "accrued_provider_cost_microusd": 0, "trial_cost_microusd": None},
        "development_controls": {"assigned": len(records), "outcomes": dict(counts),
                                 "scripted_completion_claims": claim_counts,
                                 "scored": sum(record["evaluation"] is not None for record in records),
                                 "unscored": sum(record["evaluation"] is None for record in records),
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


def verify_finished(task, arm, proposals, record, limits):
    """Independently rescore the retained selected bytes in a new verification workspace."""
    candidates = record["candidates"]
    expected_count = (1 if arm in ("lesson", "revision") and candidates and candidates[0]["public_pass"]
                      else len(proposals))
    if len(candidates) != expected_count or any(type(item["public_pass"]) is not bool for item in candidates):
        raise ValueError("The finished control lacks a scripted proposal or public observation.")
    chosen = select(arm, candidates)
    if record["effect"] != ("observed" if chosen is not None else "none"):
        raise ValueError("The control's claimed effect differs from its selection.")
    if chosen != record["selected"]:
        raise ValueError("Selection doesn't follow the frozen public rule.")
    if record["usage"]["elapsed_ns"] >= limits["elapsed_seconds"] * 1_000_000_000:
        raise ValueError("The control completed past its wall-time limit.")
    expected_calls = 1 + len(candidates) + int(chosen is not None)
    measured = record["usage"]
    if (measured["candidates"], measured["strategy_tool_calls"], measured["evaluator_calls"]) != (
        len(candidates), expected_calls, int(chosen is not None)
    ):
        raise ValueError("Resource charges differ from the completed control's operations.")
    with tempfile.TemporaryDirectory() as name:
        scratch = Path(name) / "verification"
        before = corpus.reset(task, scratch)
        initial_check = corpus.public_checks(task, (scratch / task["target"]).read_bytes())
        study = read(HERE / "study.json")
        initial = common_request(study, task, scratch, initial_check)
        if encoded(initial).decode() != record["initial_request_utf8"]:
            raise ValueError("The first request differs from the public starting subject.")
        expected_lesson = None
        for number, candidate in enumerate(candidates):
            parent = number - 1 if number and arm in ("revision", "lesson") else None
            prior = candidates[parent] if parent is not None else None
            request = request_for(study, initial, arm, prior)
            if prior is not None:
                if prior["public_pass"]:
                    raise ValueError("A revision requires an observed public failure.")
                if arm == "lesson":
                    expected_lesson = bound_lesson(task, parent, prior, json.loads(proposals[number])["lesson"])
            if candidate["parent"] != parent or candidate["request_utf8"] != encoded(request).decode():
                raise ValueError("A request or parent differs from the frozen treatment.")
        if record["lesson"] != expected_lesson:
            raise ValueError("The lesson isn't bound to this cell's observed failure.")
        result = None
        status = "missing_proposal"
        if chosen is not None:
            replacement = candidates[chosen]["utf8"].encode()
            (scratch / task["target"]).write_bytes(replacement)
            result = corpus.evaluate(task, before, scratch, replacement)
            status = "verified" if result["effect_valid"] and result["correct"] else "rejected"
        if (record["before"], record["after"], record["status"], record["evaluation"]) != (
            before, corpus.snapshot(scratch), status, result
        ):
            raise ValueError("Independent verification disagrees with the observed control.")


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
        finished = ("verified", "rejected", "missing_proposal")
        stopped = ("error", "exhausted", "timeout", "malformed_response", "refused")
        if record["status"] not in finished + stopped or record["effect"] not in ("none", "observed"):
            raise ValueError("Incomplete control. Preserve this packet.")
        if record["status"] in stopped and not record.get("reason"):
            raise ValueError("A stopped control needs its recorded reason.")
        if record["evaluation"] is not None and record["effect"] != "observed":
            raise ValueError("Scoring requires a known target effect.")
        candidates = record["candidates"]
        if len(candidates) > len(proposals):
            raise ValueError("Extra candidate in the recorded history.")
        for number, item in enumerate(candidates):
            reply = proposals[number]
            if reply.hex() != item["reply_hex"]:
                raise ValueError("Response bytes differ from the scripted control.")
            if "utf8" in item:
                parsed = parse_reply(reply, arm == "lesson" and number > 0)
                raw = parsed["replacement_utf8"].encode()
                if (parsed["claimed_complete"], raw, sha256(raw).hexdigest()) != (
                    item["claimed_complete"], item["utf8"].encode(), item["sha256"]
                ):
                    raise ValueError("A proposal or claim differs from its response.")
                if item["public_pass"] is not None and item["public_pass"] != corpus.public_checks(task, raw):
                    raise ValueError("The public observation differs from the recorded proposal.")
        if record["status"] in finished:
            verify_finished(task, arm, proposals, record, limits)
        usage = record["usage"]
        if set(usage) != {*RESOURCES, "elapsed_ns"} or any(type(value) is not int or value < 0 for value in usage.values()):
            raise ValueError("Resource counts and elapsed nanoseconds must be nonnegative integers.")
        for key in RESOURCES:
            if usage[key] > limits[key]:
                raise ValueError(f"The control exceeds its {key} allowance.")
        if any(record["usage"][key] for key in ("provider_calls", "total_tokens", "subagent_calls")):
            raise ValueError("A local script can't report provider or subagent activity.")
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

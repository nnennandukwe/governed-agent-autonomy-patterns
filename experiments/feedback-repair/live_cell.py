"""Run one frozen strategy against actual broker replies in its private cell workspace."""

import os
from pathlib import Path
import stat
import tempfile
import time

import corpus
import live_isolation
import recorder


class CellLimit(Exception):
    """Report a known resource limit reached before dispatching the next operation."""


class _Deadline(Exception):
    """Stop local work after the cell deadline without retrying an external call."""


def _target_identity(target):
    """Identify the single-link file observed by the host on this filesystem."""
    meta = target.lstat()
    if not stat.S_ISREG(meta.st_mode) or meta.st_nlink != 1:
        raise ValueError("The nominated target isn't a single regular file.")
    return {"device": meta.st_dev, "inode": meta.st_ino}


def _check_time(deadline):
    """Refuse further local work when the absolute monotonic cutoff arrives."""
    if time.monotonic() >= deadline:
        raise _Deadline()


def _place(workspace, target, replacement, deadline):
    """Replace the sole target and sync the directory before reporting the installed identity."""
    _check_time(deadline)
    with tempfile.NamedTemporaryFile(dir=workspace, delete=False) as stage:
        stage_path = Path(stage.name)
        stage.write(replacement)
        stage.flush()
        os.fsync(stage.fileno())
        identity = _target_identity(stage_path)
    try:
        _check_time(deadline)
    except _Deadline:
        stage_path.unlink()
        raise
    os.replace(stage_path, target)
    directory = os.open(workspace, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
    if _target_identity(target) != identity:
        raise ValueError("The target no longer identifies the installed file.")
    return identity


def _native_charge(usage, remaining, pricing):
    """Accept integer usage only when it fits the reservation and frozen list-price arithmetic."""
    values = [usage[key] for key in ("input_tokens", "output_tokens", "list_microusd")]
    if any(type(value) is not int or value < 0 for value in values):
        raise ValueError("Native usage must have nonnegative integer counts.")
    inputs, outputs, cost = values
    if inputs + outputs > remaining:
        raise ValueError("Native usage exceeds the remaining token reservation.")
    if cost != inputs * pricing["input_microusd_per_token"] + outputs * pricing["output_microusd_per_token"]:
        raise ValueError("Computed list cost disagrees with the frozen provider rates.")
    if usage["outcome"] not in ("complete", "refused", "incomplete"):
        raise ValueError("Unknown native response outcome.")


def _has_path(document, path):
    """Check whether each disclosed object key or array position still exists."""
    for key in path:
        if type(key) is str and type(document) is dict and key in document:
            document = document[key]
        elif type(key) is int and type(document) is list and 0 <= key < len(document):
            document = document[key]
        else:
            return False
    return True


def _mandatory(task, before, after, replacement, result):
    """Identify proven regressions and leave unmeasured private requirements unresolved."""
    for name in before:
        if name != task["target"] and after.get(name) != before[name]:
            return True, {"reason": "protected_file_changed", "file": name}
    if result["correct"]:
        return False, {"reason": "independent_expected_state_matched"}
    initial = task["initial"]
    try:
        final = corpus.decode(replacement)
    except ValueError:
        final = None
    if corpus.encode(final) == corpus.encode(initial):
        return False, {"reason": "initial_settings_semantically_unchanged"}
    for check in task["checks"]:
        path = check["path"]
        if _has_path(initial, path) and not _has_path(final, path):
            return True, {"reason": "previously_present_required_field_missing", "path": path}
        if corpus._check_path(initial, check) and not corpus._check_path(final, check):
            return True, {"reason": "previously_satisfied_public_requirement_regressed", "path": path}
    return None, {"reason": "incorrect_configuration_without_complete_preservation_evidence"}


def run_cell(private_dir, member, arm, workspace, dispatch, checkpoint, *, deadline=None):
    """Run one arm once, propagating ambiguous sends, writes, reads and checkpoint failures."""
    if arm not in recorder.ARMS:
        raise ValueError("Unknown study arm.")
    budget = recorder.read(recorder.HERE / "budget.json")
    limits = budget["per_cell_limits"]
    started = time.monotonic_ns()
    cell_deadline = started / 1_000_000_000 + limits["elapsed_seconds"]
    deadline = min(deadline, cell_deadline) if deadline is not None else cell_deadline
    workspace = Path(workspace)
    row = {"member": member, "arm": arm, "status": "started", "elapsed_ns": 0, "calls": 0,
           "input_tokens": 0, "output_tokens": 0, "list_microusd": 0, "tool_calls": 0,
           "evaluator_calls": 0, "claimed_complete": None, "mandatory_failure": None,
           "mandatory_evidence": {"reason": "no_final_observation"},
           "initial": None, "after": None, "initial_target": None, "placed_target": None,
           "effect": "none", "candidates": [], "selected": None, "evaluation": None, "lesson": None}

    def save(kind):
        """Carry observed time and the current private record to the durable controller."""
        row["elapsed_ns"] = time.monotonic_ns() - started
        checkpoint(kind, row)

    def tool():
        """Reserve one strategy tool before its check or placement."""
        _check_time(deadline)
        if row["tool_calls"] >= limits["strategy_tool_calls"]:
            raise CellLimit("strategy_tool_calls")
        row["tool_calls"] += 1

    def isolated(function, *args, scoring=False):
        """Apply this cell's deadline to an isolated local worker, preserving transport errors elsewhere."""
        _check_time(deadline)
        if scoring:
            row["evaluator_calls"] += 1
        try:
            return function(*args, deadline=deadline)
        except TimeoutError as exc:
            raise _Deadline() from exc

    def finish(status, reason=None):
        """Finalize known observations and withhold score credit when the elapsed limit was reached."""
        if row["effect"] == "none" and row["initial"] is not None and time.monotonic() < deadline:
            observed = corpus.snapshot(workspace)
            if observed != row["initial"] or _target_identity(workspace / corpus.TARGET) != row["initial_target"]:
                row.update(after=observed, mandatory_failure=True, mandatory_evidence={"reason": "unadmitted_workspace_change"})
                save("workspace_changed")
                raise ValueError("The target or protected workspace changed without an admitted placement.")
            row.update(after=observed, mandatory_failure=False, mandatory_evidence={"reason": "workspace_unchanged"})
        row["elapsed_ns"] = time.monotonic_ns() - started
        if time.monotonic() >= deadline:
            status, reason = "timeout", "elapsed_seconds"
        row["status"] = status
        if reason is not None:
            row["reason"] = reason
        return row

    save("started")
    try:
        task, before = isolated(live_isolation.prepare, private_dir, member, workspace)
        if (task["id"] != member or task["target"] != corpus.TARGET or corpus.snapshot(workspace) != before
                or before.get(corpus.TARGET) != corpus.digest(corpus.encode(task["initial"]))):
            raise ValueError("Prepared workspace or public task binding differs from the assigned member.")
        target = workspace / task["target"]
        row.update(initial=before, after=before, initial_target=_target_identity(target))
        tool()
        initial_check = corpus.public_checks(task, target.read_bytes())
        candidates = row["candidates"]
        maximum = min(1 if arm == "raw" else 2, limits["provider_calls"], limits["candidates"])
        if not maximum:
            raise CellLimit("provider_calls")
        last_refusal = None
        for number in range(maximum):
            prior = candidates[-1] if number and arm in ("revision", "lesson") else None
            history = [] if prior is None else [{"reply_hex": prior["reply_hex"], "public_pass": prior["public_pass"]}]
            request = isolated(live_isolation.candidate_request, task, arm, history, initial_check)
            remaining = limits["total_tokens"] - row["input_tokens"] - row["output_tokens"]
            if remaining <= 0:
                raise CellLimit("total_tokens")
            request_digest = corpus.digest(recorder.encoded(request))
            row["pending_request"] = {"ordinal": number + 1, "request_sha256": request_digest}
            save("request")
            _check_time(deadline)
            reply, usage = dispatch(request, remaining, deadline)
            _native_charge(usage, remaining, budget["price_snapshot"])
            if type(reply) is not bytes:
                raise ValueError("A confirmed broker reply must carry the observed text bytes.")
            row["calls"] += 1
            for key in ("input_tokens", "output_tokens", "list_microusd"):
                row[key] += usage[key]
            row["pending_request"] = None
            candidate = {"request_sha256": request_digest, "reply_hex": reply.hex(),
                         "parent": number - 1 if prior else None, "public_pass": None, "usage": dict(usage)}
            candidates.append(candidate)
            save("response")
            _check_time(deadline)
            native = usage["outcome"]
            if native == "incomplete":
                return finish("incomplete", usage["stop_reason"])
            if native == "refused":
                fields, status, reason, lesson = {}, "refused", usage["stop_reason"], None
            elif not reply:
                return finish("missing_proposal", "empty_confirmed_response")
            else:
                fields, status, reason, lesson = recorder.decode_candidate(reply, prior is not None and arm == "lesson")
            candidate.update(fields)
            if lesson is not None:
                row["lesson"] = recorder.bound_lesson(task, number - 1, prior, lesson)
            if status == "refused":
                last_refusal = reason
            elif status is None:
                tool()
                candidate["public_pass"] = corpus.public_checks(task, fields["utf8"].encode())
            action = recorder.after_reply(arm, number, status, candidate["public_pass"], maximum, maximum)
            if action == "finish":
                return finish(status, reason)
            if action == "placement":
                break
            if number + 1 == maximum:
                raise CellLimit("provider_calls")
        chosen = recorder.select(arm, candidates)
        if chosen is None:
            return finish("refused", last_refusal) if last_refusal else finish("missing_proposal")
        candidate = candidates[chosen]
        replacement = candidate["utf8"].encode()
        row.update(selected=chosen, claimed_complete=candidate["claimed_complete"])
        if corpus.snapshot(workspace) != before or _target_identity(target) != row["initial_target"]:
            raise ValueError("The selected workspace changed before replacement.")
        tool()
        row["effect"] = "uncertain"
        save("selection")
        try:
            row["placed_target"] = _place(workspace, target, replacement, deadline)
        except _Deadline:
            row["effect"] = "none"
            raise
        after = corpus.snapshot(workspace)
        if after.get(task["target"]) != candidate["sha256"]:
            raise ValueError("The observed target differs from the selected proposal.")
        row.update(effect="observed", after=after)
        save("placement")
        _check_time(deadline)
        if limits["evaluator_calls"] < 1:
            raise CellLimit("evaluator_calls")
        save("scoring")
        result = isolated(live_isolation.score, private_dir, member, before, workspace, replacement, scoring=True)
        row["evaluation"] = result
        save("evaluation")
        if type(result["correct"]) is not bool or type(result["effect_valid"]) is not bool:
            raise ValueError("Independent scoring must supply Boolean results.")
        if (result["before"], result["after"], result["selected_sha256"], result["target_sha256"]) != (
            before, after, candidate["sha256"], after[task["target"]]
        ):
            raise ValueError("Independent scoring isn't bound to the observed selection and workspace.")
        row["mandatory_failure"], row["mandatory_evidence"] = _mandatory(task, before, after, replacement, result)
        if "initial_snapshot_mismatch" in result["reasons"]:
            raise ValueError("The independent scorer's original subject differs from the enrolled subject.")
        if result["correct"] and (not result["effect_valid"] or result["reasons"] or row["mandatory_failure"]
                                  or not candidate["public_pass"] or after[task["target"]] == before[task["target"]]):
            raise ValueError("Independent scoring credited an invalid effect.")
        return finish("verified" if result["correct"] else "rejected")
    except _Deadline:
        row["pending_request"] = None
        return finish("timeout", "elapsed_seconds")
    except CellLimit as exc:
        row["pending_request"] = None
        return finish("exhausted", str(exc))

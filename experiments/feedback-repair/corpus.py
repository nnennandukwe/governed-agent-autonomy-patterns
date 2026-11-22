"""Read the frozen split, reset inert public inputs and score one selected repair."""

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import stat

ROOT = Path(__file__).resolve().parent
MAX_REPLACEMENT_BYTES = 65536
MAX_BUNDLE_BYTES = 131072
TARGET = "settings.json"
WORKSPACE_NAMES = {TARGET, "TASK.txt", "checks.json"}
SPLIT_ORDER = ["development"] * 2 + ["validation"] * 2 + ["held_out"] * 8


def digest(raw):
    """Return the SHA-256 of the exact input bytes."""
    return hashlib.sha256(raw).hexdigest()


def encode(value):
    """Encode JSON values consistently while preserving JSON types and array order."""
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"),
                       allow_nan=False) + "\n").encode("utf-8")


def _object(pairs):
    """Reject duplicate keys before a JSON object can overwrite its first value."""
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def _integer(text):
    """Admit signed integers with at most 15 decimal digits in configuration data."""
    if len(text.lstrip("-")) > 15:
        raise ValueError("integer outside corpus bounds")
    return int(text)


def _reject_number(_text):
    """Reject fractional numbers and nonfinite JSON extensions."""
    raise ValueError("unsupported numeric literal")


def decode(raw, limit=MAX_REPLACEMENT_BYTES):
    """Parse bounded UTF-8 JSON with unique keys, finite integers and valid Unicode."""
    if type(raw) is not bytes or len(raw) > limit:
        raise ValueError("JSON exceeds byte policy")
    try:
        parsed = json.loads(raw.decode("utf-8"), object_pairs_hook=_object,
                            parse_int=_integer, parse_float=_reject_number,
                            parse_constant=_reject_number)
        encode(parsed)
    except RecursionError as error:
        raise ValueError("excessive JSON nesting") from error
    return parsed


def _keys(value, expected):
    """Require the exact fields for the requested frozen record."""
    if type(value) is not dict or value.keys() != set(expected):
        raise ValueError("unexpected corpus fields")


def _hex(value, width):
    """Recognize a lowercase fixed-width public identifier or commitment."""
    return type(value) is str and re.fullmatch("[0-9a-f]{%d}" % width, value) is not None


def _read_file(directory_fd, name, limit, private=False):
    """Read a single regular file without following aliases or accepting changing bytes."""
    descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory_fd)
    with os.fdopen(descriptor, "rb") as handle:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_size > limit:
            raise ValueError("unsafe or oversized file")
        if private and (before.st_uid != os.geteuid() or stat.S_IMODE(before.st_mode) != 0o600):
            raise ValueError("private file requires owner custody")
        raw = handle.read(limit + 1)
        after = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
        identity = ("st_dev", "st_ino", "st_mode", "st_uid", "st_nlink", "st_ctime_ns")
        if len(raw) != before.st_size or any(getattr(before, field) != getattr(after, field)
                                             for field in identity):
            raise ValueError("file changed during observation")
        return raw


def _open_directory(path, private=False):
    """Open the final directory itself and enforce private custody when requested."""
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    if private:
        metadata = os.fstat(descriptor)
        if metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) != 0o700:
            os.close(descriptor)
            raise ValueError("private directory requires owner custody")
    return descriptor


def _index():
    """Verify split shape, order, commitments and unique membership before path lookup."""
    directory_fd = _open_directory(ROOT)
    try:
        index = decode(_read_file(directory_fd, "split.json", MAX_BUNDLE_BYTES), MAX_BUNDLE_BYTES)
    finally:
        os.close(directory_fd)
    return _index_entries(index)


def _index_entries(index):
    """Require exactly one frozen instance per distinct family ID across the three splits."""
    _keys(index, {"schema", "members"})
    entries = index["members"]
    if index["schema"] != "feedback-repair-split/1" or type(entries) is not list:
        raise ValueError("unsupported split")
    for entry in entries:
        _keys(entry, {"id", "split", "sha256"})
        if not _hex(entry["id"], 32) or not _hex(entry["sha256"], 64):
            raise ValueError("invalid member ID or commitment")
    if ([entry["split"] for entry in entries] != SPLIT_ORDER
            or len({entry["id"] for entry in entries}) != len(SPLIT_ORDER)
            or len({entry["sha256"] for entry in entries}) != len(SPLIT_ORDER)):
        raise ValueError("invalid split membership")
    return entries


def members():
    """Return the frozen split and opaque family ID of every proposed assignment."""
    return [(entry["split"], entry["id"]) for entry in _index()]


def _check_path(document, check):
    """Compare a public assertion to a typed value reached through objects and arrays."""
    for component in check["path"]:
        if type(component) is str and type(document) is dict and component in document:
            document = document[component]
        elif (type(component) is int and type(document) is list
              and 0 <= component < len(document)):
            document = document[component]
        else:
            return False
    if "starts_with" in check:
        return type(document) is str and document.startswith(check["starts_with"])
    return encode(document) == encode(check["equals"])


def public_checks(task, target_bytes):
    """Check only the disclosed field assertions against an inert proposal."""
    try:
        proposal = decode(target_bytes)
    except ValueError:
        return False
    return type(proposal) is dict and all(
        _check_path(proposal, check) for check in task["checks"]
    )


def read_member(directory, entry):
    """Open the committed bytes of one host-selected bundle and validate its task schema."""
    _keys(entry, {"id", "split", "sha256"})
    if not _hex(entry["id"], 32) or not _hex(entry["sha256"], 64) or entry["split"] not in SPLIT_ORDER:
        raise ValueError("invalid member locator")
    private = entry["split"] != "development"
    directory_fd = _open_directory(directory, private)
    try:
        raw = _read_file(directory_fd, entry["id"] + ".json", MAX_BUNDLE_BYTES, private)
    finally:
        os.close(directory_fd)
    if digest(raw) != entry["sha256"]:
        raise ValueError("member commitment mismatch")
    task = decode(raw, MAX_BUNDLE_BYTES)
    _keys(task, {"schema", "id", "target", "family", "prompt", "checks", "initial", "expected",
                 "salt" if private else "decoy"})
    if (task["schema"] != "feedback-repair-task/1" or task["id"] != entry["id"]
            or task["target"] != TARGET or (private and not _hex(task["salt"], 64))
            or type(task["family"]) is not str or not re.fullmatch("[a-z][a-z-]{1,63}", task["family"])
            or type(task["prompt"]) is not str or not 1 <= len(task["prompt"]) <= 2048
            or type(task["checks"]) is not list or not 1 <= len(task["checks"]) <= 8):
        raise ValueError("invalid task binding")
    for check in task["checks"]:
        if type(check) is dict and "starts_with" in check:
            _keys(check, {"path", "starts_with"})
            if type(check["starts_with"]) is not str or not check["starts_with"]:
                raise ValueError("invalid public string prefix")
        else:
            _keys(check, {"path", "equals"})
        path = check["path"]
        if type(path) is not list or not 1 <= len(path) <= 16 or not all(
                type(component) is str or (type(component) is int and component >= 0)
                for component in path):
            raise ValueError("invalid public assertion")
    if type(task["initial"]) is not dict:
        raise ValueError("invalid replacement fixture")
    initial = _initial_files(task)[TARGET]
    for field in ("expected",) if private else ("expected", "decoy"):
        if type(task[field]) is not dict or len(encode(task[field])) > MAX_REPLACEMENT_BYTES:
            raise ValueError("invalid replacement fixture")
    if encode(task["expected"]) == initial:
        raise ValueError("repair must change initial state")
    if (public_checks(task, initial)
            or not public_checks(task, encode(task["expected"]))):
        raise ValueError("public assertion must expose part of the requested repair")
    if not private and (not public_checks(task, encode(task["decoy"]))
                        or encode(task["decoy"]) == encode(task["expected"])):
        raise ValueError("public decoy must pass its check and differ from the expected repair")
    return task


def controls():
    """Load the two public development cases with their authored gold and decoy bytes."""
    result = []
    for entry in _index():
        if entry["split"] == "development":
            task = read_member(ROOT / "fixtures", entry)
            result.append((task, encode(task["expected"]), encode(task["decoy"])))
    return result


def verify(private_dir=None):
    """Check public membership and optionally open all ten original private commitments."""
    entries = _index()
    public = ROOT / "fixtures"
    private_dir = Path(private_dir) if private_dir is not None else None
    if private_dir is not None:
        resolved = private_dir.resolve(strict=True)
        repository = ROOT.parents[1]
        if resolved == repository or repository in resolved.parents:
            raise ValueError("private members must stay outside the checkout")
    families = set()
    counts = Counter()
    for directory, private in ((public, False), (private_dir, True)):
        if directory is None:
            continue
        assigned = [entry for entry in entries if (entry["split"] != "development") == private]
        directory_fd = _open_directory(directory, private)
        try:
            if set(os.listdir(directory_fd)) != {entry["id"] + ".json" for entry in assigned}:
                raise ValueError("missing or unexpected member file")
        finally:
            os.close(directory_fd)
        for entry in assigned:
            task = read_member(directory, entry)
            if task["family"] in families:
                raise ValueError("family appears in more than one assignment")
            families.add(task["family"])
            counts[private] += 1
    return {"public_verified": counts[False], "private_verified": counts[True],
            "proposed_members": len(entries)}


def _initial_files(task):
    """Render each starting file and enforce the byte limit used to observe it."""
    files = {TARGET: encode(task["initial"]), "TASK.txt": (task["prompt"] + "\n").encode(),
             "checks.json": encode(task["checks"])}
    if any(len(raw) > MAX_REPLACEMENT_BYTES for raw in files.values()):
        raise ValueError("initial workspace file exceeds byte policy")
    return files


def reset(task, workspace):
    """Populate a new empty directory exclusively and reread its initial file digests."""
    files = _initial_files(task)
    workspace = Path(workspace)
    workspace.mkdir(mode=0o700, exist_ok=True)
    directory_fd = _open_directory(workspace)
    try:
        if os.listdir(directory_fd):
            raise ValueError("reset requires an empty directory")
        for name, raw in files.items():
            target_fd = os.open(name, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600, dir_fd=directory_fd)
            with os.fdopen(target_fd, "wb") as handle:
                handle.write(raw)
    finally:
        os.close(directory_fd)
    return snapshot(workspace)


def _observe(workspace):
    """Independently open the three allowed entries after selection has closed."""
    directory_fd = _open_directory(workspace)
    try:
        if set(os.listdir(directory_fd)) != WORKSPACE_NAMES:
            raise ValueError("workspace entry set changed")
        return {name: _read_file(directory_fd, name, MAX_REPLACEMENT_BYTES)
                for name in sorted(WORKSPACE_NAMES)}
    finally:
        os.close(directory_fd)


def _digests(files):
    """Commit to each observed file by name and exact bytes."""
    return {name: digest(raw) for name, raw in files.items()}


def snapshot(workspace):
    """Return exact file digests from the host's scoped workspace."""
    return _digests(_observe(workspace))


def evaluate(task, before_snapshot, workspace, selected_bytes):
    """Score the final observed files and bind that outcome to the selected proposal bytes."""
    reasons = []
    initial = _digests(_initial_files(task))
    if before_snapshot != initial:
        reasons.append("initial_snapshot_mismatch")
    try:
        observed = _observe(workspace)
    except (OSError, ValueError):
        observed = {}
        reasons.append("workspace_unreadable_or_out_of_scope")
    after = _digests(observed)
    if any(after.get(name) != initial[name] for name in WORKSPACE_NAMES - {TARGET}):
        reasons.append("protected_bytes_changed_or_missing")
    supplied = type(selected_bytes) is bytes and len(selected_bytes) <= MAX_REPLACEMENT_BYTES
    selected_digest = digest(selected_bytes) if supplied else None
    target_digest = after.get(TARGET)
    if not supplied:
        reasons.append("no_admissible_selected_bytes")
    elif observed.get(TARGET) != selected_bytes:
        reasons.append("target_differs_from_selection")
    if target_digest == initial[TARGET]:
        reasons.append("no_observed_target_change")
    effect_valid = not reasons
    semantic_match = False
    if TARGET in observed:
        try:
            semantic_match = encode(decode(observed[TARGET])) == encode(task["expected"])
        except ValueError:
            pass
    if not semantic_match:
        reasons.append("final_configuration_incorrect")
    return {"correct": effect_valid and semantic_match, "effect_valid": effect_valid,
            "reasons": reasons, "selected_sha256": selected_digest, "target_sha256": target_digest,
            "before": before_snapshot, "after": after}


def main():
    """Verify public inputs or the custodian's original private files and print counts."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["verify"])
    parser.add_argument("--private-dir", type=Path, help="owner-only directory outside the checkout")
    arguments = parser.parse_args()
    try:
        counts = verify(arguments.private_dir)
    except (OSError, ValueError) as error:
        parser.exit(1, "Corpus verification failed (%s).\n" % type(error).__name__)
    print(json.dumps(counts, sort_keys=True))


if __name__ == "__main__":
    main()

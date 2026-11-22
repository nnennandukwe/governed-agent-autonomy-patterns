"""Freeze, collect and inspect the single authorized private Experiment B run."""

import argparse
from collections import Counter
from contextlib import contextmanager
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import sqlite3
import stat
import statistics
import subprocess
import sys
import tempfile
import time

import corpus
import live_cell
import live_isolation
import live_provider
import recorder


HERE = Path(__file__).resolve().parent
RUN_ID = "gaap-feedback-b-2026-11-21"
CAP = 25_000_000
BASE_INPUTS = {
    "study.json": "04ba163f9bc5edcc64e3c31c046b2053528dbc41a1445c280e9ff304c6fcf4eb",
    "split.json": "0acc81c13b98c3b9161fef41cb8d261b832e7599d0d6fc9b4c23cd62885c699d",
    "budget.json": "36e66140246817d0658d58f5cc5e45de7e2970079f7a79bcb0b514e7f7dafd37",
}


class Hold(RuntimeError):
    """Preserve the run and prevent further dispatch until its evidence is resolved."""


def digest(raw):
    """Hash exact bytes for later independent comparison."""
    return hashlib.sha256(raw).hexdigest()


def utc():
    """Return a UTC timestamp suitable for the billing window."""
    return datetime.now(timezone.utc).isoformat()


def identity():
    """Require a read-only mounted study image and bind its actual backing bytes."""
    if sys.platform != "darwin" or not os.statvfs(HERE).f_flag & os.ST_RDONLY:
        raise Hold("Run the frozen study from its read-only macOS image.")
    details = plistlib.loads(subprocess.check_output(["/usr/bin/hdiutil", "info", "-plist"]))
    paths = [Path(item["image-path"]) for item in details["images"]
             if any(entry.get("mount-point") == str(HERE) for entry in item["system-entities"])]
    if len(paths) != 1:
        raise Hold("The running image has no unique backing file.")
    revision = (HERE / "REVISION").read_text().strip()
    if not re.fullmatch("[a-f0-9]{40}", revision):
        raise Hold("The runner revision is missing.")
    sources = {str(path.relative_to(HERE)): digest(path.read_bytes())
               for pattern in ("*.py", "*.sh", "*.json", "*.md", "fixtures/*.json", "REVISION")
               for path in sorted(HERE.glob(pattern)) if path.is_file()}
    if any(sources.get(key) != value for key, value in BASE_INPUTS.items()):
        raise Hold("The approved study, split or pricing input has changed.")
    with paths[0].open("rb") as source:
        image_hash = hashlib.file_digest(source, "sha256").hexdigest()
    return {"revision": revision, "image_sha256": image_hash, "sources": sources,
            "profile": live_isolation.profile(),
            "python_sha256": digest(Path(sys.executable).resolve().read_bytes()),
            "python_version": sys.version, "sqlite_version": sqlite3.sqlite_version}


def private_home(private_dir):
    """Resolve the single cooperating custodian location and refuse aliases."""
    private_dir = Path(private_dir).absolute()
    if private_dir != private_dir.resolve(strict=True):
        raise Hold("Custody must use its original directory without aliases.")
    shared = private_dir.parent
    for path in (private_dir, shared):
        meta = path.stat()
        if meta.st_uid != os.getuid() or not stat.S_ISDIR(meta.st_mode):
            raise Hold("The study and its custody parent must belong to the operator.")
    if stat.S_IMODE(private_dir.stat().st_mode) != 0o700 or shared.stat().st_mode & 0o022:
        raise Hold("Custody must be private and its parent owner-controlled.")
    return shared / RUN_ID


@contextmanager
def ledger(home, *, create=False):
    """Lock the run directory for the lifetime of the writer and open an intact database."""
    if create:
        home.mkdir(mode=0o700, exist_ok=False)
    meta = home.lstat()
    if (not stat.S_ISDIR(meta.st_mode) or meta.st_uid != os.getuid()
            or stat.S_IMODE(meta.st_mode) != 0o700):
        raise Hold("The run directory must be private and operator-owned.")
    directory = os.open(home, os.O_DIRECTORY | os.O_NOFOLLOW | os.O_RDONLY)
    try:
        try:
            fcntl.flock(directory, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise Hold("Another process owns this run.") from error
        path = home / "ledger.sqlite3"
        if create:
            os.close(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600))
        meta = path.lstat()
        if (not stat.S_ISREG(meta.st_mode) or meta.st_nlink != 1
                or stat.S_IMODE(meta.st_mode) != 0o600 or meta.st_uid != os.getuid()):
            raise Hold("The ledger isn't a private single-link file.")
        connection = sqlite3.connect(path.as_uri() + "?mode=rw", uri=True, isolation_level="IMMEDIATE")
        try:
            connection.row_factory = sqlite3.Row
            connection.execute("PRAGMA foreign_keys = ON")
            connection.execute("PRAGMA synchronous = FULL")
            connection.execute("PRAGMA fullfsync = ON")
            if connection.execute("PRAGMA quick_check").fetchone()[0] != "ok":
                raise Hold("The ledger failed its consistency check.")
            yield connection
        finally:
            connection.close()
    finally:
        os.close(directory)


def initialize(db, freeze):
    """Commit the immutable freeze and all assigned cells before allowing any call."""
    db.executescript("""
        CREATE TABLE metadata (name TEXT PRIMARY KEY, value BLOB NOT NULL);
        CREATE TABLE cells (
            id INTEGER PRIMARY KEY, split TEXT NOT NULL, member TEXT NOT NULL, arm TEXT NOT NULL,
            state TEXT NOT NULL, started TEXT, ended TEXT, last_step TEXT, evidence BLOB, hold_reason TEXT,
            UNIQUE(member,arm));
        CREATE TABLE calls (
            cell INTEGER REFERENCES cells(id), ordinal INTEGER, state TEXT NOT NULL,
            reserved BLOB NOT NULL, wire BLOB NOT NULL, started TEXT NOT NULL,
            http_status INTEGER, request_id TEXT, headers BLOB, body BLOB, ended TEXT, usage BLOB,
            PRIMARY KEY(cell,ordinal));
    """)
    with db:
        db.execute("INSERT INTO metadata VALUES ('freeze',?)", (recorder.encoded(freeze),))
        db.executemany("INSERT INTO cells(id,split,member,arm,state) VALUES (?,?,?,?,'unattempted')",
                       [(i, split, member, arm) for i, (split, member, arm)
                        in enumerate(freeze["assignments"], start=1)])


def freeze_run(private_dir):
    """Qualify the host, commit the exact inputs and return the external freeze digest."""
    home = private_home(private_dir)
    runtime = identity()
    counts = corpus.verify(private_dir)
    if counts != {"private_verified": 10, "public_verified": 2, "proposed_members": 12}:
        raise Hold("Twelve original families must pass verification.")
    isolation = live_isolation.qualify()
    assignments = [[split, member, arm] for split, member in corpus.members() for arm in recorder.ARMS]
    initial = {}
    with tempfile.TemporaryDirectory() as scratch:
        for _, member in corpus.members():
            workspace = Path(scratch) / member
            projected, _ = live_isolation.prepare(private_dir, member, workspace)
            passed = corpus.public_checks(projected, (workspace / projected["target"]).read_bytes())
            requests = [live_isolation.candidate_request(projected, arm, [], passed) for arm in recorder.ARMS]
            hashes = {digest(recorder.encoded(request)) for request in requests}
            if len(hashes) != 1:
                raise Hold("The first request differs between arms.")
            initial[member] = hashes.pop()
    frozen = {"id": RUN_ID, "runtime": runtime, "qualified": isolation, "assignments": assignments,
              "initial_requests": initial, "custody": str(private_dir), "counts": counts,
              "spending_limit_microusd": CAP, "limits": recorder.read(HERE / "budget.json")["study_limits"],
              "decision_rule": recorder.read(HERE / "study.json")["outcome"], "frozen_at": utc()}
    with ledger(home, create=True) as db:
        initialize(db, frozen)
        directory = os.open(home, os.O_DIRECTORY | os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    parent = os.open(home.parent, os.O_DIRECTORY | os.O_RDONLY)
    try:
        os.fsync(parent)
    finally:
        os.close(parent)
    return digest(recorder.encoded(frozen))


def frozen_record(db, expected):
    """Match the digest supplied by the owner before opening any live execution path."""
    record = db.execute("SELECT value FROM metadata WHERE name='freeze'").fetchone()
    if not record:
        raise Hold("Enrollment is incomplete.")
    raw = record[0]
    if digest(raw) != expected:
        raise Hold("The supplied freeze digest doesn't match the enrolled study.")
    frozen = json.loads(raw)
    enrolled = [list(row) for row in db.execute("SELECT split,member,arm FROM cells ORDER BY id")]
    if enrolled != frozen["assignments"] or len(enrolled) != 48:
        raise Hold("The ledger differs from the 48 frozen assignments.")
    return frozen


def checkpoint(db, cell, kind, record):
    """Persist private cell observations and selected bytes at each effect boundary."""
    with db:
        if db.execute("UPDATE cells SET last_step=?, evidence=? WHERE id=? AND state='running'",
                      (kind, recorder.encoded(record), cell)).rowcount != 1:
            raise Hold("Only the active cell can publish an observation.")


def charges(db, cell=None):
    """Count all native charges and retain worst-case reservations for unsettled calls."""
    clause = "" if cell is None else " WHERE cell=?"
    params = () if cell is None else (cell,)
    sums = {"requests": 0, "input_tokens": 0, "output_tokens": 0, "list_microusd": 0}
    for row in db.execute("SELECT state,reserved,usage FROM calls" + clause, params):
        source = json.loads(row["usage"] if row["state"] == "settled" else row["reserved"])
        sums["requests"] += 1
        for key in ("input_tokens", "output_tokens", "list_microusd"):
            sums[key] += source[key]
    return sums


def account_binding():
    """Bind the initial operator-designated account to the serving credential used throughout collection."""
    workspace, key = live_provider.read_account_scope()
    return {"workspace_id": workspace, "key_id": key,
            "credential_fingerprint": live_provider.serving_credential_fingerprint(scope=(workspace, key))}


def dispatch(db, cell, request, remaining_tokens, deadline, initial_digest):
    """Reserve before send and commit the exact service response before decoding it."""
    owner = db.execute("SELECT state FROM cells WHERE id=?", (cell,)).fetchone()
    if not owner or owner["state"] != "running":
        raise Hold("Dispatch requires a running assignment.")
    used = charges(db, cell)
    ordinal = used["requests"] + 1
    if ordinal == 1 and digest(recorder.encoded(request)) != initial_digest:
        raise Hold("The first public input changed after freeze.")
    if remaining_tokens != 12_000 - used["input_tokens"] - used["output_tokens"]:
        raise Hold("Token accounting differs from the native ledger.")
    stored = db.execute("SELECT value FROM metadata WHERE name='billing_scope'").fetchone()
    if not stored:
        raise Hold("The serving key hasn't been enrolled.")
    scope = json.loads(stored[0])
    try:
        wire, reservation = live_provider.prepare_message(request, remaining_tokens, deadline,
                                                         expected_fingerprint=scope["credential_fingerprint"])
    except live_provider.ReservationError as error:
        raise live_cell.CellLimit("total_tokens") from error
    with db:
        if any(reservation[key] != value for key, value in scope.items()):
            raise Hold("The native token count belongs to a different billing scope.")
        total = charges(db)
        if (ordinal > 2 or total["requests"] >= 96
                or reservation["total_tokens"] > remaining_tokens
                or total["input_tokens"] + total["output_tokens"] + reservation["total_tokens"] > 576_000
                or total["list_microusd"] + reservation["list_microusd"] > CAP):
            raise live_cell.CellLimit("reservation")
        earlier = db.execute("""SELECT wire FROM calls JOIN cells ON cells.id=calls.cell
            WHERE member=(SELECT member FROM cells WHERE id=?) AND ordinal=1 LIMIT 1""", (cell,)).fetchone()
        if ordinal == 1 and earlier and earlier[0] != wire:
            raise Hold("The paired first requests aren't byte-identical.")
        if time.monotonic() >= deadline:
            raise live_cell.CellLimit("elapsed_seconds")
        db.execute("""INSERT INTO calls(cell,ordinal,state,reserved,wire,started)
            VALUES (?,?,'reserved',?,?,?)""", (cell, ordinal, recorder.encoded(reservation), wire, utc()))
    status, headers, body = live_provider.send_message(wire, deadline, reservation=reservation)
    request_id = headers.get("request-id")
    with db:
        db.execute("""UPDATE calls SET state='received',http_status=?,request_id=?,headers=?,body=?,ended=?
            WHERE cell=? AND ordinal=?""", (status, request_id, recorder.encoded(headers), body, utc(), cell, ordinal))
    if request_id and db.execute("SELECT count(*) FROM calls WHERE request_id=?", (request_id,)).fetchone()[0] != 1:
        raise Hold("The provider returned a previously observed request ID.")
    reply, usage = live_provider.parse_response(status, headers, body, reservation)
    with db:
        db.execute("UPDATE calls SET state='settled',usage=? WHERE cell=? AND ordinal=?",
                   (recorder.encoded(usage), cell, ordinal))
    return reply, usage


def collect(db, private_dir, home, frozen, *, designation=None):
    """Run unattempted assignments serially and hold any interrupted cell without replay."""
    if frozen["runtime"] != identity() or str(private_dir) != frozen["custody"]:
        raise Hold("The runtime or custody location differs from the freeze.")
    if db.execute("SELECT 1 FROM cells WHERE state='running'").fetchone():
        raise Hold("An interrupted cell holds this run. Inspect the private ledger before recovery.")
    todo = db.execute("SELECT * FROM cells WHERE state='unattempted' ORDER BY id").fetchall()
    if not todo:
        return
    expected_root = digest(recorder.encoded(frozen))
    if (not isinstance(designation, dict) or set(designation) != {"freeze_sha256", "workspace_id", "api_key_id"}
            or designation["freeze_sha256"] != expected_root):
        raise Hold("Supply the billing custodian's account designation for this exact freeze.")
    scope = account_binding()
    if (scope["workspace_id"], scope["key_id"]) != (designation["workspace_id"], designation["api_key_id"]):
        raise Hold("The serving environment differs from the designated billing workspace or key.")
    with db:
        db.execute("INSERT OR IGNORE INTO metadata VALUES ('designation',?)", (recorder.encoded(designation),))
        if json.loads(db.execute("SELECT value FROM metadata WHERE name='designation'").fetchone()[0]) != designation:
            raise Hold("The billing custodian's original designation changed.")
        db.execute("INSERT OR IGNORE INTO metadata VALUES ('billing_scope',?)", (recorder.encoded(scope),))
        if json.loads(db.execute("SELECT value FROM metadata WHERE name='billing_scope'").fetchone()[0]) != scope:
            raise Hold("The provider billing scope changed after activation.")
    if corpus.verify(private_dir) != frozen["counts"]:
        raise Hold("The committed inputs changed after freeze.")
    elapsed_ns = sum(json.loads(row[0])["elapsed_ns"] for row in
                     db.execute("SELECT evidence FROM cells WHERE state='finished'"))
    limit_ns = frozen["limits"]["elapsed_seconds"] * 1_000_000_000
    for row in todo:
        remaining_ns = limit_ns - elapsed_ns
        if remaining_ns <= 0:
            raise Hold("The aggregate trial time has elapsed.")
        deadline = (time.monotonic_ns() + remaining_ns) / 1_000_000_000
        cell = row["id"]
        with db:
            db.execute("UPDATE cells SET state='running',started=? WHERE id=? AND state='unattempted'",
                       (utc(), cell))
        workspace = home / f"workspace-{cell:02d}"
        def submit(request, remaining_tokens, deadline):
            """Bind the provider request to the single active assignment."""
            return dispatch(db, cell, request, remaining_tokens, deadline, frozen["initial_requests"][row["member"]])
        def observe(kind, record):
            """Write a cell observation to the same durable owner as its requests."""
            checkpoint(db, cell, kind, record)
        try:
            result = live_cell.run_cell(private_dir, row["member"], row["arm"], workspace,
                                        submit, observe, deadline=deadline)
        except (RuntimeError, ValueError, OSError) as error:
            # Keep prior evidence intact, including possibly dispatched requests and placements.
            with db:
                db.execute("UPDATE cells SET hold_reason=? WHERE id=?", (type(error).__name__, cell))
            raise
        elapsed_ns += result["elapsed_ns"]
        actual = charges(db, cell)
        if (result["calls"], result["input_tokens"], result["output_tokens"], result["list_microusd"]) != (
                actual["requests"], actual["input_tokens"], actual["output_tokens"], actual["list_microusd"]):
            raise Hold("Cell charges disagree with the provider receipts.")
        with db:
            db.execute("UPDATE cells SET state='finished',ended=?,last_step='closed',evidence=? WHERE id=?",
                       (utc(), recorder.encoded(result), cell))


def assignment_rows(db):
    """Expose measured outcomes and resource counts without publishing private candidates or tasks."""
    rows = []
    for row in db.execute("SELECT * FROM cells ORDER BY id"):
        evidence = json.loads(row["evidence"]) if row["evidence"] else {}
        result = {key: row[key] for key in ("id", "member", "arm", "split", "state", "started", "ended", "hold_reason")}
        for key in ("elapsed_ns", "claimed_complete", "mandatory_failure", "effect", "tool_calls", "evaluator_calls"):
            result[key] = evidence.get(key)
        result["outcome"] = evidence.get("status") if row["state"] == "finished" else None
        result["last_step"] = row["last_step"]
        result["selected"] = evidence.get("selected")
        result["mandatory_evidence"] = evidence.get("mandatory_evidence")
        result["score_reasons"] = (evidence.get("evaluation") or {}).get("reasons")
        result["candidate_count"] = len(evidence.get("candidates", []))
        result["maximum_committed_usage"] = charges(db, row["id"])
        rows.append(result)
    return rows


def measures(rows, billed_at_list):
    """Report the denominator and uncertainty for each outcome in one split and arm."""
    attempted = [row for row in rows if row["state"] != "unattempted"]
    finished = [row for row in rows if row["state"] == "finished"]
    scored = [row for row in finished if row["outcome"] in ("verified", "rejected")]
    claimed = [row for row in attempted if row["claimed_complete"] is True]
    false = sum(row["outcome"] == "rejected" for row in claimed)
    verified = sum(row["outcome"] == "verified" for row in scored)
    costs = [row["maximum_committed_usage"]["list_microusd"] for row in finished]
    durations = [row["elapsed_ns"] for row in finished]
    return {
        "assigned": len(rows), "attempted": len(attempted), "finished": len(finished), "scored": len(scored),
        "verified": verified, "verified_per_attempt": verified / len(attempted) if attempted else None,
        "outcomes": dict(Counter(row["outcome"] for row in finished)),
        "claimed": len(claimed), "unclaimed": sum(row["claimed_complete"] is False for row in attempted),
        "unknown_claim": sum(row["claimed_complete"] is None for row in attempted),
        "unresolved_claim": sum(row not in scored for row in claimed),
        "false_completions": false, "false_completions_per_claim": false / len(claimed) if claimed else None,
        "false_completions_per_attempt": false / len(attempted) if attempted else None,
        "mandatory_failures": sum(row["mandatory_failure"] is True for row in attempted),
        "mandatory_unknown": sum(row["mandatory_failure"] is None for row in attempted),
        "open_assignments": sum(row["state"] == "running" for row in attempted),
        "mandatory_disposition": ("reject" if any(row["mandatory_failure"] is True for row in attempted)
                                  else "unresolved" if any(row["mandatory_failure"] is None for row in rows)
                                  else "no_observed_violation"),
        "uncertain_placements": sum(row["effect"] == "uncertain" for row in attempted),
        "median_elapsed_ns_finished": statistics.median(durations) if durations else None,
        "median_list_microusd_finished": statistics.median(costs) if costs else None,
        "median_billed_microusd_finished": statistics.median(costs) if costs and billed_at_list else None,
    }


def decision(rows, billing):
    """Apply the frozen exploratory rule only to complete paired and attributable observations."""
    held = [row for row in rows if row["split"] == "held_out"]
    arms = {arm: [row for row in held if row["arm"] == arm] for arm in recorder.ARMS}
    lesson = arms["lesson"]
    if any(row["mandatory_failure"] is True for row in rows if row["arm"] == "lesson"):
        return {"state": "reject", "reason": "observed new mandatory failure in the lesson strategy"}
    comparison = lesson + arms["raw"] + arms["resampling"]
    if (any(len(arms[arm]) != 8 for arm in ("raw", "lesson", "resampling"))
            or any(row["outcome"] not in ("verified", "rejected", "refused") for row in comparison)
            or any(row["mandatory_failure"] is None for row in comparison)):
        return {"state": "unresolved", "reason": "required held-out comparisons are missing or uncertain"}
    if not billing or not billing["all_rows_at_list"]:
        return {"state": "unresolved", "reason": "the billed cost of each paired cell isn't established"}
    summaries = {arm: measures(values, True) for arm, values in arms.items()}
    observed = summaries["lesson"]
    raw, resample = summaries["raw"], summaries["resampling"]
    differences = {"additional_repairs_vs_raw": observed["verified"] - raw["verified"],
                   "additional_repairs_vs_resampling": observed["verified"] - resample["verified"],
                   "median_cost_delta_microusd": observed["median_billed_microusd_finished"]
                                                 - resample["median_billed_microusd_finished"],
                   "median_latency_delta_ns": observed["median_elapsed_ns_finished"]
                                              - resample["median_elapsed_ns_finished"]}
    passes = (differences["additional_repairs_vs_raw"] >= 2 and differences["additional_repairs_vs_resampling"] >= 2
              and differences["median_cost_delta_microusd"] <= 0 and differences["median_latency_delta_ns"] <= 0)
    return {"state": "consider_larger_test" if passes else "threshold_not_met", **differences}


def report(db, frozen):
    """Produce a private readback with every assigned cell, including ambiguous and unattempted work."""
    rows = assignment_rows(db)
    pending = db.execute("SELECT count(*) FROM calls WHERE state!='settled'").fetchone()[0]
    settled = dict.fromkeys(("requests", "input_tokens", "output_tokens", "list_microusd"), 0)
    for raw, in db.execute("SELECT usage FROM calls WHERE state='settled'"):
        receipt = json.loads(raw)
        settled["requests"] += 1
        for key in ("input_tokens", "output_tokens", "list_microusd"):
            settled[key] += receipt[key]
    record = db.execute("SELECT value FROM metadata WHERE name='billing'").fetchone()
    billing = json.loads(record[0]) if record else None
    if billing:
        billing.pop("capture")
        live_provider.reconcile_billing(billing, settled)
    billed_at_list = billing is not None and billing["all_rows_at_list"]
    for row in rows:
        row["billed_microusd"] = row["maximum_committed_usage"]["list_microusd"] if billed_at_list else None
    states = dict.fromkeys(("unattempted", "running", "finished"), 0)
    states.update(Counter(row["state"] for row in rows))
    return {
        "study": frozen["id"], "freeze_sha256": digest(recorder.encoded(frozen)), "states": states,
        "assignments": rows, "settled_usage": settled, "maximum_committed_usage": charges(db),
        "unsettled_reservations": pending, "billing": billing,
        "finished_elapsed_ns": sum(row["elapsed_ns"] for row in rows if row["state"] == "finished"),
        "outcomes": {split: {arm: measures([row for row in rows if (row["arm"], row["split"]) == (arm, split)],
                                           billed_at_list) for arm in recorder.ARMS}
                     for split in ("development", "validation", "held_out")},
        "false_blocks": {"designated_legal_controls": 0, "rate": None},
        "escapes": {"designated_exposed_opportunities": 0, "rate": None},
        "decision": decision(rows, billing),
    }


def billing_interval(db):
    """Select the enrolled provider dimension and observed time span for the complete run."""
    unfinished = db.execute("SELECT count(*) FROM cells WHERE state!='finished'").fetchone()[0]
    unresolved = db.execute("SELECT count(*) FROM calls WHERE state!='settled'").fetchone()[0]
    scope = db.execute("SELECT value FROM metadata WHERE name='billing_scope'").fetchone()
    start, end = db.execute("SELECT min(started),max(ended) FROM calls").fetchone()
    if unfinished or unresolved or not scope or not start or not end:
        raise Hold("Final billing needs all 48 dispositions and settled call receipts.")
    binding = json.loads(scope[0])
    return (binding["workspace_id"], binding["key_id"], datetime.fromisoformat(start), datetime.fromisoformat(end))


def reconcile(db, capture=None, trusted_sha256=None):
    """Commit an independently reconciled final report without changing the original receipts."""
    expected = charges(db)
    native = live_provider.fetch_billing(*billing_interval(db), time.monotonic() + 90,
                                         capture=capture, trusted_sha256=trusted_sha256)
    live_provider.reconcile_billing(native, expected)
    with db:
        db.execute("INSERT OR IGNORE INTO metadata VALUES ('billing',?)", (recorder.encoded(native),))
        original = json.loads(db.execute("SELECT value FROM metadata WHERE name='billing'").fetchone()[0])
        for key in ("requests", "input_tokens", "output_tokens", "list_microusd", "billed_cents"):
            if original[key] != native[key]:
                raise Hold("A revised native charge conflicts with the stored billing observation.")


def read_designation(path):
    """Read the custodian's bounded account handoff under the original private-file rules."""
    directory = corpus._open_directory(path.parent, private=True)
    try:
        return corpus.decode(corpus._read_file(directory, path.name, 2048, private=True), 2048)
    finally:
        os.close(directory)


def main():
    """Run a single operator action and keep all provider bodies, credentials and answers private."""
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    for name in ("freeze", "run", "status", "billing-window", "reconcile"):
        sub = actions.add_parser(name)
        sub.add_argument("--private-dir", type=Path, required=True)
        if name != "freeze":
            sub.add_argument("--expected-root", required=True)
        if name == "run":
            sub.add_argument("--account-designation", type=Path)
        if name == "reconcile":
            sub.add_argument("--capture", type=Path)
            sub.add_argument("--trusted-sha256")
    capture = actions.add_parser("capture-billing")
    capture.add_argument("--workspace-id", required=True)
    capture.add_argument("--key-id", required=True)
    capture.add_argument("--started-at", required=True, type=datetime.fromisoformat)
    capture.add_argument("--ended-at", required=True, type=datetime.fromisoformat)
    capture.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    os.umask(0o077)
    if args.action == "capture-billing":
        raw = live_provider.capture_billing(args.workspace_id, args.key_id, args.started_at, args.ended_at,
                                            time.monotonic() + 90)
        with args.output.open("xb") as out:
            out.write(raw)
        print(digest(raw))
        return
    private = args.private_dir.absolute()
    if args.action == "freeze":
        print(freeze_run(private))
        return
    with ledger(private_home(private)) as db:
        frozen = frozen_record(db, args.expected_root)
        if args.action == "run":
            designation = read_designation(args.account_designation) if args.account_designation else None
            collect(db, private, private_home(private), frozen, designation=designation)
        elif args.action == "billing-window":
            workspace, key, start, end = billing_interval(db)
            print(json.dumps({"workspace_id": workspace, "key_id": key,
                              "started_at": start.isoformat(), "ended_at": end.isoformat()}))
            return
        elif args.action == "reconcile":
            if bool(args.capture) != bool(args.trusted_sha256):
                raise Hold("Supply both the independent capture and its trusted digest.")
            if args.capture and args.capture.stat().st_size > live_provider.MAX_CAPTURE:
                raise Hold("The billing capture exceeds the admitted size.")
            reconcile(db, args.capture.read_bytes() if args.capture else None, args.trusted_sha256)
        print(json.dumps(report(db, frozen), sort_keys=True, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (Hold, OSError, sqlite3.Error, ValueError, RuntimeError) as error:
        sys.exit(f"Study held. {error}")

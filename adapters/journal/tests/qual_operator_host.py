"""Provision the supervisor-owned source index and current operator grants."""

import contextlib
import fcntl
import json
import os

from qual_host import canonical, digest, identity, write_private
from qual_submit_host import sync_dir


class Operator:
    """Act as the test supervisor that owns operator grants, writer leases and private observer deposits."""

    def __init__(self, slot, state, health):
        """Provision separate authority and evidence directories, bind them to the enrolled slot and pin
        its conversion source.
        """
        self.slot = slot
        self.commands = []
        self.legacy_host = dict(slot.host)
        self.authority = health / "operator-authority"
        self.candidates = state / "observer-deposits"
        for directory in (self.authority, self.candidates):
            directory.mkdir(mode=0o700)
        self.lock = self.authority / "authority.lock"
        self.writers = self.authority / "writers.lock"
        for path in (self.lock, self.writers):
            write_private(path, b"")
        ids = {k: slot.host["slot"][k] for k in ("workspace_id", "store_id", "profile_digest")}
        self.profile = dict(ids, format="gaap.shipping-reconciliation/0.2",
                            effect_observer=slot.host["profile"]["effect_observer"])
        self.profile_digest = digest(canonical(self.profile))
        self.config = dict(profile=self.profile, authority=identity(self.authority),
                           authority_lock=identity(self.lock), candidates=identity(self.candidates),
                           writers_lock=identity(self.writers), entry_socket_cookie=None, conversion=None)
        slot.host["reconciliation"] = self.config
        self.rights = {name: dict(read=True, convert=allowed, reconcile=allowed)
                       for name, allowed in (("alice", True), ("bob", False), ("operator2", True))}
        self.publish_rights()
        if slot.journal.exists():
            self.pin_source()
        else:
            slot.configure()

    def publish_rights(self):
        """Replace and sync the current grants while holding the supervisor's exclusive authority lock."""
        record = {k: self.profile[k] for k in ("workspace_id", "store_id")}
        record.update(format="gaap.reconciliation-authority/0.2",
                      reconciliation_profile_digest=self.profile_digest, principals=self.rights)
        with self.lock.open("rb") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            stage = self.authority / "next.json"
            write_private(stage, canonical(record))
            stage.replace(self.authority / "operator.json")
            sync_dir(self.authority)

    def pin_source(self):
        """Bind the supervisor handoff to the anchor's current generation and the exact bytes of both source files."""
        anchor = self.slot.anchor.read_bytes()
        self.config["conversion"] = dict(source_generation=json.loads(anchor)["generation"],
            source_anchor_digest=digest(anchor), source_journal_digest=digest(self.slot.journal.read_bytes()))
        self.slot.configure()

    @contextlib.contextmanager
    def old_handoff(self):
        """Expose the original host configuration to the released client and restore the current handoff on exit."""
        current, path = self.slot.host, self.slot.config
        self.slot.host = self.legacy_host
        self.slot.config = path.with_name("released-host.json")
        self.slot.configure()
        try:
            yield
        finally:
            self.slot.host, self.slot.config = current, path

    def deposit(self, record):
        """Create a private file for the canonical observer record and return its digest after syncing
        the file and directory.
        """
        raw = canonical(record)
        checksum = digest(raw)
        path = self.candidates / (checksum[7:] + ".json")
        with path.open("xb") as stream:
            os.fchmod(stream.fileno(), 0o600)
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        sync_dir(self.candidates)
        return checksum

    def command(self, *args, expected, **options):
        """Run a fresh CLI process, check its expected status and record the arguments, principal and reply."""
        result = self.slot.call(*args, expected=expected, **options)
        self.commands.append({"argv": list(args), "principal": self.slot.host["principal"],
                              "result": result})
        return result

    def convert(self):
        """Require the converter to acknowledge the source pinned by this supervisor."""
        return self.command("convert", expected="converted")

    def reconcile(self, attempt, source, expected="recorded", **options):
        """Submit only the attempt ID and deposited source digest, and check the operator's expected status."""
        return self.command("reconcile", attempt, source, expected=expected, **options)

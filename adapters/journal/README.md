# Local journal enrollment, lookup and operator reconciliation

A trusted host can enroll one private workspace and read a previous submission after losing a response. Enrollment creates generation zero. A lookup can return the caller's original sealed receipt, its partial evidence, a conflict, or confirmed absence. A workspace hold applies across all callers. The separate shipping reference can submit one foreground file replacement under the same journal lock. [Accepted storage contract](../../docs/contracts/journal-enrollment-v0.1.md) · [Recovery decisions](../../docs/adr/0009-durable-agent-run-reconciliation.md)

## Build and run

Build the ordinary binary with the pinned toolchain. Use its help to check the command forms.

```sh
cargo build --locked --manifest-path adapters/journal/Cargo.toml
adapters/journal/target/debug/gaap-journal --help
```

The host launches exactly one operation per process. The command accepts these arguments after the host installs descriptors 3 and 4.

```text
gaap-journal enroll
gaap-journal lookup KEY
gaap-journal lookup KEY --proposal-stdin
gaap-journal convert
gaap-journal reconcile ATTEMPT_ID EVIDENCE_DIGEST
```

With `--proposal-stdin`, send the full local-file proposal as JSON on standard input. The reader compares its canonical bytes with the saved proposal. Read the single JSON object on standard output, including on a refusal. An optional `warning` records a second failure during cleanup. A nonzero exit indicates a request, access, storage or delivery error. An absent receipt after response loss calls for lookup. Neither a successful read nor the process exit code grants new execution authority.

## Recover an uncertain shipping effect

A workspace operator can admit the enrolled host observer's original evidence after an interrupted shipping invocation. Start by confirming which slot, source generation and digest pair the supervisor authorized. For a new workspace, enroll once and withhold shipping grants until conversion finishes. A deployed slot needs its own conversion authorization. The command changes the format in place. Released v0.1 clients subsequently refuse it. [Operator contract](../../docs/contracts/journal-reconciliation-v0.2.md)

1. Provision the supervisor's reconciliation binding on fd 3. Pin the historical profile, the host observer's separate private archive, current workspace rights, permanent authority lock and writer lease. The supervisor holds a shared writer lease from before launch until every possible committer stops. Every worker, including one launched with a v0.1 binary, must retain that lease across launcher death. Serialize policy changes under the authority lock. The observer has sole deposit authority over its archive.
2. Run `gaap-journal convert` under current conversion rights with the expected source generation and both canonical source digests in the trusted handoff. A lost reply permits the same authorized conversion to reconfirm its origin. Keep both files and the volume health state if the pair can't be confirmed.
3. Run `gaap-journal reconcile ATTEMPT_ID EVIDENCE_DIGEST` with current read and reconciliation rights. The attempt ID has 32 lowercase hex digits. The digest names a retained observer deposit. The operator reports a disposition only after the pending intake and ruling have reached their separate commit points. An interrupted operator can retry the same pair. A successor needs their own current rights.
4. Use the ordinary private lookup to read the original run and its separate effect status. A workspace dispute blocks all fresh submissions. An old receipt stays sealed and an unsealed dispatch stays unsealed. Preserve missing or damaged evidence for diagnosis. A healthy volume alone can't clear a pending or disputed effect.

In the trusted `reconciliation` object, `candidates` pins the observer directory on the enrolled volume. `authority`, `authority_lock` and `writers_lock` pin the independent supervisor directory and its two lock files. The directory holds canonical `operator.json` with the slot IDs, reconciliation profile digest and per-principal read, convert and reconcile permissions. `conversion` pins the source generation and both digests. A host that observes entry also pins `entry_socket_cookie` to the worker's one-use Unix packet socket on fd 5. To establish non-entry, start the watcher before the supervised launch and retain the bound entry request, the withheld permit and coverage through definitive stop. The optional gate alone can't establish a file outcome. [Host binding](src/recovery.rs) · [Disposable supervisor example](tests/qual_operator_host.py) · [Witness records](tests/qual_operator_observer.py)

## Provision the host handoff

1. Reserve a never-enrolled workspace in the host's trusted registry. Choose one supported local ext4 mount with ordered data journaling and write barriers. The Linux service needs descriptor-relative `openat2`, mount information from procfs, and permission to read the ext4 driver's effective options and kernel device identity. Create three separate 0700 directories for the workspace, control and journal. Their absolute paths must have no symlink components. Create the permanent empty 0600 lock and the canonical unused anchor in the control directory. Sync the files, directories and newly created directory entries before handing the slot to GAAP. Preserve the registry, paths, identities and original profile across restarts.
2. Provision a one-byte 0600 volume health file on a separate device with the initial ASCII byte `0`. Durably bind that same health authority to every operation on the volume. The writer sets and syncs a pending byte (`P`) before a storage operation that can mutate disk state. On success it can restore `0`. Any error returned by file sync, directory sync or checked writer close changes the marker to `1`. This covers delayed space and quota errors as well as a read-only filesystem. Ordinary capacity errors before publication preserve the prior store. Both `P` and `1` hold the volume across process restarts. Every cooperating supervisor must take the same health-file lock before changing it. If the adapter reports `health_authority_failed_withdraw_volume`, the supervisor must withdraw the volume independently, including after restart. Requalification and reinstatement belong to the host operator. The supervisor must monitor volume health independently, including read-side faults that could kill a process before it reports an error. A healthy lookup keeps the latch read-only. Use persistent independent storage for deployment. The process suite uses temporary storage for its isolated cases.
3. Authenticate the caller and check workspace rights in the supervisor. Pass an independently provisioned, read-only regular configuration file on fd 3 and the existing read/write health file on fd 4. Both files must have a single link, private mode 0600 and service-account ownership. Keep configuration immutable for the invocation. The service account and this launcher form a trust boundary. Callers can supply their submission key and bounded proposal.

The host configuration is a closed JSON object. Its `principal`, `can_enroll` and `can_read` fields express current authorization. Its `slot` object contains the workspace and store IDs, the original profile digest, Linux mount ID, and the five identity records named `workspace`, `control`, `store`, `lock` and `health`. Each identity has `path,dev,ino,uid,mode`. Device and inode are unsigned decimal strings. Mode contains the permission bits as an integer. The control anchor retains the four identities on the journal volume. The provisioner pins health independently. Host configuration input is bounded at 32 MiB.

The `profile` object selects the local-file decoder, original policy, capability, tool schema, sandbox, executor, actor IDs and grant requirements. The profile digest covers its complete JCS bytes. The independently trusted `archive.entries` index maps each opaque locator to its actor ID and original evidence bytes as lowercase hex. New historical evidence belongs in that archive under its retained trust policy. A journal-supplied locator can only select an existing entry. The [profile and archive types](src/history/types.rs), [evidence checks](src/history/evidence.rs) and [test provisioner](tests/qual_host.py) give the exact encodings. The [populated examples](tests/qual_history.py) build records around actual reference requests and receipts.

For a trusted, single-use Python launcher, open both existing files before replacing descriptors 3 and 4. This snippet takes the paths, executable and authenticated command arguments from the supervisor.

```python
import fcntl
import os

def launch(config_path, health_path, executable, args):
    def open_existing(path, flags):
        original = os.open(path, flags | os.O_NOFOLLOW | os.O_CLOEXEC)
        try:
            return fcntl.fcntl(original, fcntl.F_DUPFD_CLOEXEC, 5)
        finally:
            os.close(original)

    config = open_existing(config_path, os.O_RDONLY)
    health = open_existing(health_path, os.O_RDWR)
    os.dup2(config, 3, inheritable=True)
    os.dup2(health, 4, inheritable=True)
    os.execv(executable, [executable, *args])
```

## Interpret a reply

| Status | Meaning and next action |
| --- | --- |
| `enrolled` | The slot is active. Repeated enrollment confirms the same binding. |
| `absent`, `conflict`, `partial`, `published` | The reader validated the confirmed generation. Only the matched principal can receive its attempt. Check `workspace_hold` even for a missing key. A partial run has no terminal receipt. A published receipt retains its separate effect evidence. |
| `not_enrolled`, `incomplete_enrollment` | A trusted unused slot can be explicitly enrolled. An interrupted reservation needs operator diagnosis. |
| `unconfirmed`, `invalid_store`, `unsupported_storage`, `unavailable` | Preserve the files. Inspect the host binding, volume health or archived evidence before any recovery decision. |
| `forbidden`, `busy`, `capacity`, `invalid_request` | Use the current authorized principal, wait for the writer to exit, check limits or correct the input as indicated by `reason`. |

## Submit a journal-bound shipping proposal

The shipping reference's `journal-submit KEY` command consumes the complete local-file proposal on stdin. An authenticated supervisor launches it on Linux with the same fd 3 configuration and fd 4 health descriptor as the reader. The caller supplies only the key and proposal. Read the result through `gaap-journal lookup KEY` in a new process, including after the original connection closes. [Shipping reference](../../examples/submitted-proposal/) · [Execution and recovery plan](../../docs/plans/issue-32-journal-executor.md)

1. Provision a dedicated shipping workspace. Its three fixture files are `shipping.py`, `TASK.md` and `deployment.json`. Use the subject digest of their shipping snapshot, including each file's identity. Keep this workspace outside the legacy session layout. Include the current principal and read permission in the normal configuration.
2. In the optional `live` configuration, set `can_submit` and pin three identities. The `authority` directory and its permanent empty 0600 `authority_lock` file reside on separate host storage from the journal volume. The independent 0700 `archive` directory resides on the qualified volume outside the workspace, control and journal directories. An inspector can read this archive with `can_submit` set to false.
3. The trusted grant issuer binds the current slot, principal, key, proposal digest, subject, budget and validity interval in a private canonical `authority.json`. Before setting `allow_retention`, the issuer checks that the exact proposal can be stored without retaining credentials. A different proposal digest or a missing retention grant stops admission. Its grant names the `filesystem.write:shipping.py` scope and includes the original approval evidence. The issuer takes the permanent authority lock exclusively before replacing the record. It syncs the file and directory before acknowledging an update. The worker checks the source under a shared lock and holds it through the target rename. Withdraw a damaged or unconfirmed source until the issuer establishes its state. [Authority record and checks](src/live/authority.rs)
4. Launch the production shipping reference using that trusted handoff. A fresh authorized request records intent, then records the actual engine's dispatch gates before staging the target. The live witness observes the move and binds it to the staged inode under sole-writer exclusion. Actual verifier evidence and original sealed outputs remain separate from the target operation. The worker archives evidence before publishing references to it.
5. If the process exits without a reply, start the ordinary journal reader with current read rights and the original archive. An identical retry returns the saved facts. A changed proposal for the same key returns `conflict`. A different key on a workspace with an unresolved effect returns `workspace_held` on fresh submission. Preserve that workspace for operator review.

The private archive contains bounded, immutable records for the original grant, observed effect, executor and verifier. Each history read loads the archive under the journal lock. Keep the archive with the enrolled slot. Its evidence can explain a visible write even when the original run failed verification. Only the original live engine can produce the terminal receipt. After interruption, an unsealed history stays unsealed.

The [disposable Linux host fixture](tests/qual_submit_host.py) shows how to build the three-file subject, bind a real approval and launch both commands with independent descriptors. In a provisioned supervisor, a normal submission and read use these arguments.

```sh
gaap-submitted-proposal-reference journal-submit repair-shipping < proposal.json
gaap-journal lookup repair-shipping
```

## Reproduce qualification

The required journal CI job compiles the normal binary and a separate `qualification` binary with test-only fault hooks. On a disposable ordinary-user Ubuntu runner it mounts a fresh ext4 image, then runs the actual CLI with independent readers, writers and observers. The job retains platform and binary identities, syscall ordering, per-case receipts and independent file measurements. Run the same [qualification entry point](qualify-ext4.sh) with the normal binary, instrumented binary, submitted-proposal reference binary and an evidence directory. Supply the qualification shipping binary as the fifth argument to run the integrated submission cases. Its elevated operations mount and unmount the disposable test disk. The service processes run as the ordinary account.

These tests cover process death, duplicate enrollment, lost delivery, faults around sync and publication, damaged and full histories, and read-only return of existing receipts. Process termination leaves the kernel running. Hardware power-loss behavior requires separate storage qualification. The production binary excludes the fault hooks.

The same Linux job qualifies the submitted-proposal reference executor at its own file rename. LLDB stops the foreground process immediately before or after that operation. Independent observations cover the target, the reference locks, the process exit and a duplicate submission after restart. The [slice 2 qualification plan](../../docs/plans/issue-32-executor-quiescence.md) identifies the additional proof a journal-bound executor needs.

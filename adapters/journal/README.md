# Local journal enrollment and lookup

A trusted host can enroll one private workspace and read a previous submission after losing a response. Enrollment creates generation zero. A lookup can return the caller's original sealed receipt, its partial evidence, a conflict, or confirmed absence. A workspace hold applies across all callers. The service has no command to dispatch an effect or append an attempt. [Accepted storage contract](../../docs/contracts/journal-enrollment-v0.1.md) · [Recovery decisions](../../docs/adr/0009-durable-agent-run-reconciliation.md)

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
```

With `--proposal-stdin`, send the full local-file proposal as JSON on standard input. The reader compares its canonical bytes with the saved proposal. Read the single JSON object on standard output, including on a refusal. An optional `warning` records a second failure during cleanup. A nonzero exit indicates a request, access, storage or delivery error. An absent receipt after response loss calls for lookup. Neither a successful read nor the process exit code grants new execution authority.

## Provision the host handoff

1. Reserve a never-enrolled workspace in the host's trusted registry. Choose one supported local ext4 mount with ordered data journaling and write barriers. The Linux service needs descriptor-relative `openat2`, mount information from procfs, and permission to read the ext4 driver's effective options and kernel device identity. Create three separate 0700 directories for the workspace, control and journal. Their absolute paths must have no symlink components. Create the permanent empty 0600 lock and the canonical unused anchor in the control directory. Sync the files, directories and newly created directory entries before handing the slot to GAAP. Preserve the registry, paths, identities and original profile across restarts.
2. Provision a one-byte 0600 volume health file on a separate device with the initial ASCII byte `0`. Durably bind that same health authority to every operation on the volume. The writer sets and syncs a pending byte (`P`) before a storage operation that can mutate disk state. On success it can restore `0`. A reported writeback error changes the marker to `1`. Both `P` and `1` hold the volume across process restarts. Every cooperating supervisor must take the same health-file lock before changing it. If the adapter reports `health_authority_failed_withdraw_volume`, the supervisor must withdraw the volume independently, including after restart. Requalification and reinstatement belong to the host operator. The supervisor must monitor volume health independently, including read-side faults that could kill a process before it reports an error. A healthy lookup keeps the latch read-only. Use persistent independent storage for deployment. The process suite uses temporary storage for its isolated cases.
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

## Reproduce qualification

The required journal CI job compiles the normal binary and a separate `qualification` binary with test-only fault hooks. On a disposable ordinary-user Ubuntu runner it mounts a fresh ext4 image, then runs the actual CLI with independent readers, writers and observers. The job retains platform and binary identities, syscall ordering, per-case receipts and independent file measurements. Run the same [qualification entry point](qualify-ext4.sh) with the normal binary, instrumented binary, submitted-proposal reference binary and an evidence directory. Its elevated operations mount and unmount the disposable test disk. The service processes run as the ordinary account.

These tests cover process death, duplicate enrollment, lost delivery, faults around sync and publication, damaged and full histories, and read-only return of existing receipts. Process termination leaves the kernel running. Hardware power-loss behavior requires separate storage qualification. The production binary excludes the fault hooks.

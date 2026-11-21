# Journal enrollment and historical lookup implementation plan

## Summary

After a client loses a response, the host needs to read verified evidence for the original principal and key without executing anything. The maintainer accepted the storage contract at `092a17c` and authorized slice 1. [Decision on #32](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/32#issuecomment-7069218948) · [Storage contract](../contracts/journal-enrollment-v0.1.md)

## Skills To Use

Use Software Build Plan and Qodo rules for the new adapter, transaction-proof and workflow-invariants for the journal states, failure-path-testing for the process matrix, and CLI parity for command behavior. Use voice-canon and both Qodo review skills before delivery.

## Scope

Add an independently built Rust adapter and a host-invoked CLI. Enrollment writes one empty journal for a pre-provisioned slot. Lookup interprets a fully validated historical generation. Execution, admission of populated histories, operator rulings and approval continuation stay with their separate decisions in [ADR 0009](../adr/0009-durable-agent-run-reconciliation.md).

## Package Layout / File-to-Task Mapping

The Linux storage module owns file descriptors, locks, staging, publication and storage holds. The history module owns the closed fact grammar and frozen contract validators. The command boundary owns the authenticated host handoff, access checks, bounded input and JSON replies. The process suite launches the command against real Linux ext4. The CI job retains the results and the final check requires that job. The adapter guide describes the provisioner's responsibilities.

## Dependencies And Settings

Use the repository's core validators, Serde, JCS and SHA-256. Isolate Linux filesystem bindings in the adapter. The host provisions three existing private directories, a permanent lock and the unused anchor. It separately supplies a read-only configuration descriptor, the authenticated principal, rights, pinned identities, the original profile, a trusted evidence index and a persistent volume-health latch outside the journal volume. Current callers receive only the CLI request fields. The service owner must withdraw a failed health authority from service. Use only local test infrastructure and the repository's existing hosted Actions environment.

## Canonical Contracts

The accepted journal contract specifies the closed disk shape, identity and digest domains, relation between the three anchor phases and the journal, capacity limits and lookup outcomes. Validate every record before returning any principal's result. The host's current rights decide access. Historical policy, executor, capability and witness identities come from the independently retained profile. New records aren't a trust source. Exact duplicates compare canonical bytes, and unresolved effects hold the whole workspace.

## TDD And BDD Implementation Strategy

Start with an externally provisioned unused slot, launch enrollment, and read a missing key from a second process. Repeat enrollment and inspect both directories. Add competing processes and interruptions at each publication boundary. Use a separately generated linked request/result/receipt and independent file observations for populated cases. Exercise lost delivery and malformed or conflicting histories through the same reader.

## Component Design

| Persistent state | Owning action and commit | Next observable state |
| --- | --- | --- |
| Unused anchor, empty store | The authorized exclusive writer syncs its staged reserved anchor, renames it, then syncs the control directory. | Only that same live reservation owner may create the journal. A restarted writer holds a visible reservation. |
| Reserved | Stage and sync canonical generation zero, close it, publish with no replacement and sync the store directory. Reopen and validate bytes and pins. | The reader reports incomplete enrollment until activation. |
| Reserved, exact journal | Sync a staged active anchor, rename it, then sync the control directory. | The rename makes matching evidence visible. The final directory sync permits acknowledgment. |
| Active, exact history | A shared reader validates the whole generation. Repeat enrollment under exclusive lock rechecks and confirms durability. | Keyed answers describe existing facts. A lost response permits lookup. |
| Conflicting, missing, incompatible or unconfirmed | Preserve entries. The writer persists a pending marker on the independent device before risky disk operations. An interrupted marker or a writeback fault holds the volume. | Hold. A subsequent successful sync alone can't clear a reported storage fault. |

Only the writer that owns a staged entry can remove it. It must recheck file and directory identity before a mutation or cleanup. The lock stays stable across all replacements. Clearing a quarantine, migration and repairing an incomplete reservation require operator action outside this slice.

## Failure And Recovery Rules

| Operation and fault | Surviving state, reply and qualification |
| --- | --- |
| Authorize, open, stat, check mount and take lock | Refuse wrong caller, identity, alias, link, unsafe mode or mount before publication. An occupied lock returns busy. Race two enrollments and interleave a changed binding. |
| Read, bound, parse, validate | Missing or unreadable state holds. Reject damaged bytes, unknown types, changed IDs, cross-record mismatches and invalid historical evidence anywhere in the snapshot. Measure original files before and after lookup. |
| Exclusive create at each stage | A collision preserves the existing entry. The writer can't adopt it. |
| Write or flush | Short writes must complete or fail before publication. If writing fails, preserve prior canonical content and conditionally clean only the owned stage. |
| File sync or close | Failure prevents that rename. Report EIO to the host's independent durable health authority. Inject failure separately for reserve, journal and active stages. Kill after a returned EIO and before the latch update. A surviving pending marker blocks independent lookup and enrollment. |
| Rename and first no-replace publication | Test before and after each rename with real process termination. Failure before activation can't make the journal authoritative. Another entry at the journal name survives. |
| Directory sync after each rename | No success acknowledgment on failure. A reader can see an unacknowledged rename on a healthy filesystem after process death. A reported writeback fault requires quarantine. |
| Independent reread, final response, cleanup | A failed integrity check prevents activation. Kill after the final sync and check repeat enrollment. Preserve ambiguous or foreign stages. Cleanup failure after a commit retains the commit and reports a warning. |

## Commit Plan

Deliver the accepted contract status, adapter, operator instructions and qualification lane as one reviewed implementation. Preserve the separate worktree and all changes in the reference checkout.

## Branch And PR Flow

Use `feat/journal-enrollment-lookup` from `092a17c`. Run complete-diff Qodo Local Review on the final commit, publish the PR, and require a completed structured review of that same head and all selected CI jobs. Check the SHA at merge and compare the merged and reviewed trees. Record issue status after post-merge checks.

## Test Plan

Run formatter, Clippy, locked build and tests, contract artifact parity, the existing recovery model and lane selector. Run the new populated-history tests and Linux qualification suite on a private ext4 volume with ordered journaling and barriers. Independent processes exercise locks, SIGKILL, storage faults, response loss and read-only access. Record actual observer results, checksums and qualified platform. Process termination leaves the kernel running. A hardware power-loss guarantee requires separate testing.

## Definition Of Done

Enrollment and the independent reader agree on empty, partial and published states. Invalid inputs and incomplete enrollment preserve evidence and prevent a keyed success. A verified PR, its merged tree and post-merge CI establish delivery. #32 retains the executor and operator follow-ups.

## Assumptions And Defaults

The provisioner, current authenticator, retained profile, archive and volume-health authority are trusted. The contract assumes a dedicated service account and cooperating writers. The adapter provides no protection against a hostile process with that account or restoration of the whole trusted installation.

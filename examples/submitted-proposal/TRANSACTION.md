# Local proposal commit and recovery checks

The canonical local record is `operator/state.json`. Its replacement publishes one full ledger snapshot. File mutation and ledger publication have separate commit points, so the host records an in-flight marker before effect dispatch.

| State | Commit or transition | Observable state and recovery |
| --- | --- | --- |
| Prepared | Validate the strict input, current session, subject and canonical requests under the session lock. | No effect; reject malformed input without a marker. |
| Admitted | Sync and rename the ledger containing `inflight`. | A restart refuses dispatch while that marker remains. |
| Admission durability refused | Publish an abort snapshot before the engine starts. | No effect has run; a successful abort clears the marker. A failed abort leaves the visible state for inspection. |
| Snapshot byte limit exceeded | Refuse publication before creating a temporary ledger. | Keep the prior readable snapshot. After an effect, its retained in-flight marker blocks replay. |
| Staged | Create, write, chmod, flush and sync a temporary file; recheck the source snapshot. | A pre-rename failure preserves the current target and produces a failed effect. |
| Effect committed | Rename the staged file over the allowed target. | The write exists even if verification or later publication fails. |
| Result published | Sync and rename the full ledger with the result. | Inspection revalidates canonical receipts and evidence; duplicates return that receipt. |
| Uncertain | A post-effect observation or result-publication operation fails. | Preserve the target and marker; refuse another dispatch. Retain this root and inspect it. |
| Published with warning | Directory sync fails after the result rename. | Return the visible result with a durability warning; don't report rollback. |

The writer always rechecks observed ownership and subject before replacement. It never rolls back over later operator edits. Unrecognized schema, changed capability trust, changed policy, relocated directories, symlinks, hardlinks and tampered evidence are rejected without repairing or deleting the stored state. The reference assumes a controlled trusted process and local filesystem; hostile concurrent path replacement and power-loss continuation need separate designs.

The migrated Rust fault matrix exercises dispatch/abort/result create, write, flush, file sync, rename and directory sync; target create, write, mode change, flush, file sync and rename; post-commit uncertainty; concurrent edits; and verifier drift. Identity, open/read, parse and validation refusal paths use actual links, moved roots, malformed proposals and edited records. Each case inspects actual files and the production ledger consumer. A byte-limit regression checks that oversized snapshot publication leaves the previous state readable and permits a fresh admission when no effect ran. Initial directory-creation failures preserve an incomplete root for operator inspection; no auto-cleanup or uninstall operation is provided.

Verifier regressions reject a function that misses the two-item case and a function that memorizes the published examples. They also exercise repeated squaring from both rounding and Boolean comparisons, confirming that intermediate arithmetic stays in the float domain. Host-seeded samples are repeatable from the recorded request digest.

The Python lifecycle checks initialize, inspect, submit, restart, read historical outcomes and repeat delivery through the public CLI/MCP surfaces. The original launcher-specific selection, inspector and recipe-publication tests stay with the preserved conference checkout. This port carries their relevant saved-history and no-reset behavior through the new CLI; it doesn't introduce another launcher or UI.

Run the matrix with:

```bash
cargo test --manifest-path examples/submitted-proposal/Cargo.toml --locked
python3 -B -m unittest discover -s examples/submitted-proposal/tests -p 'test_*.py'
```

An unknown outcome is an inspection requirement. The host won't continue, reconcile or replay that effect automatically. Issue #32 owns the durable continuation design.
